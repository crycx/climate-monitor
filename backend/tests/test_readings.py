import os
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

# Ensure tests never depend on external PostgreSQL.
os.environ["DATABASE_URL"] = "sqlite:///:memory:"
os.environ["SECRET_KEY"] = "test-secret"
os.environ["SESSION_COOKIE_SECURE"] = "false"
os.environ["EXPOSE_RESET_TOKEN_IN_RESPONSE"] = "true"

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import run as app_module  # noqa: E402
from app.models import Device, Location, Reading, User  # noqa: E402


app = app_module.app
db = app_module.db


def setup_function(_function):
    with app.app_context():
        db.drop_all()
        db.create_all()


def _register(client, username: str, email: str, password: str = "secret123"):
    return client.post(
        "/api/auth/register",
        json={"username": username, "email": email, "password": password},
    )


def test_create_reading_uses_devices_current_location_id():
    with app.app_context():
        user = User(username="sensor-user", email="sensor@example.com", password_hash="hash")
        db.session.add(user)
        db.session.flush()

        location = Location(user_id=user.id, name="Kasvuhoone")
        db.session.add(location)
        db.session.flush()

        device = Device(
            user_id=user.id,
            location_id=location.id,
            name="ESP32-C3",
            device_type="esp32",
            api_key="test-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

        device_id = device.id
        location_id = location.id

    client = app.test_client()
    response = client.post(
        "/api/readings",
        headers={"X-API-Key": "test-api-key"},
        json=[{"sensor_type": "temperature", "value": 23.5, "unit": "C"}],
    )

    assert response.status_code == 201

    with app.app_context():
        saved = db.session.execute(db.text("SELECT device_id, location_id FROM readings")).first()

    assert saved is not None
    assert saved.device_id == device_id
    assert saved.location_id == location_id


def test_legacy_device_readings_post_endpoint_saves_reading():
    with app.app_context():
        user = User(username="legacy-user", email="legacy@example.com", password_hash="hash")
        db.session.add(user)
        db.session.flush()

        location = Location(user_id=user.id, name="Legacy Room")
        db.session.add(location)
        db.session.flush()

        device = Device(
            user_id=user.id,
            location_id=location.id,
            name="Legacy ESP32",
            device_type="esp32",
            api_key="legacy-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

        device_id = device.id

    client = app.test_client()
    response = client.post(
        f"/api/devices/{device_id}/readings",
        headers={"X-API-Key": "legacy-api-key"},
        json={"sensor_type": "humidity", "value": 57.2, "unit": "%"},
    )

    assert response.status_code == 201
    assert response.get_json()["saved"] == 1


def test_latest_readings_endpoint_returns_user_scope_data():
    owner_client = app.test_client()
    outsider_client = app.test_client()

    owner_register = _register(owner_client, "owner", "owner@example.com")
    owner_id = owner_register.get_json()["id"]
    _register(outsider_client, "outsider", "outsider@example.com")

    with app.app_context():
        location = Location(user_id=owner_id, name="Owner Room")
        db.session.add(location)
        db.session.flush()

        device = Device(
            user_id=owner_id,
            location_id=location.id,
            name="Owner ESP32",
            device_type="esp32",
            api_key="owner-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

    ingest = owner_client.post(
        "/api/readings",
        headers={"X-API-Key": "owner-api-key"},
        json=[{"sensor_type": "temperature", "value": 21.9, "unit": "C"}],
    )
    assert ingest.status_code == 201

    latest_owner = owner_client.get("/api/readings/latest")
    assert latest_owner.status_code == 200
    owner_payload = latest_owner.get_json()
    assert len(owner_payload) == 1
    assert owner_payload[0]["sensor_type"] == "temperature"

    latest_outsider = outsider_client.get("/api/readings/latest")
    assert latest_outsider.status_code == 200
    assert latest_outsider.get_json() == []


def test_latest_readings_supports_location_period_and_after_id_filters():
    client = app.test_client()
    register = _register(client, "filter-user", "filter-user@example.com")
    user_id = register.get_json()["id"]

    with app.app_context():
        loc_a = Location(user_id=user_id, name="A")
        loc_b = Location(user_id=user_id, name="B")
        db.session.add_all([loc_a, loc_b])
        db.session.flush()

        device_a = Device(
            user_id=user_id,
            location_id=loc_a.id,
            name="A-device",
            device_type="esp32",
            api_key="filter-api-a",
            is_active=True,
        )
        device_b = Device(
            user_id=user_id,
            location_id=loc_b.id,
            name="B-device",
            device_type="esp32",
            api_key="filter-api-b",
            is_active=True,
        )
        db.session.add_all([device_a, device_b])
        db.session.flush()

        now_utc = datetime.now(timezone.utc)
        today_start = datetime(now_utc.year, now_utc.month, now_utc.day)
        yesterday_mid = today_start - timedelta(hours=12)
        today_mid = today_start + timedelta(hours=12)

        older = Reading(
            device_id=device_a.id,
            location_id=loc_a.id,
            sensor_type="temperature",
            value=20.1,
            unit="C",
            timestamp=yesterday_mid,
        )
        newer_same_location = Reading(
            device_id=device_a.id,
            location_id=loc_a.id,
            sensor_type="temperature",
            value=21.2,
            unit="C",
            timestamp=today_mid,
        )
        newer_other_location = Reading(
            device_id=device_b.id,
            location_id=loc_b.id,
            sensor_type="temperature",
            value=22.3,
            unit="C",
            timestamp=today_mid,
        )
        db.session.add_all([older, newer_same_location, newer_other_location])
        db.session.commit()

        older_id = older.id
        loc_a_id = loc_a.id

    filtered = client.get(
        f"/api/readings/latest?location_id={loc_a_id}&after_id={older_id}&order=asc"
    )
    assert filtered.status_code == 200
    payload = filtered.get_json()
    assert len(payload) == 1
    assert payload[0]["location_id"] == loc_a_id

    yesterday_only = client.get("/api/readings/latest?period=yesterday")
    assert yesterday_only.status_code == 200
    day_payload = yesterday_only.get_json()
    assert len(day_payload) == 1
    assert day_payload[0]["id"] == older_id


def test_latest_readings_rejects_period_with_explicit_time_range():
    client = app.test_client()
    _register(client, "bad-filter-user", "bad-filter-user@example.com")

    response = client.get(
        "/api/readings/latest?period=yesterday&from_ts=2026-04-04T00:00:00Z"
    )
    assert response.status_code == 400


def test_create_reading_with_location_updates_device_location_and_reuses_existing_location():
    with app.app_context():
        user = User(username="location-user", email="location@example.com", password_hash="hash")
        db.session.add(user)
        db.session.flush()

        old_location = Location(user_id=user.id, name="garage")
        new_location = Location(user_id=user.id, name="greenhouse")
        db.session.add_all([old_location, new_location])
        db.session.flush()

        device = Device(
            user_id=user.id,
            location_id=old_location.id,
            name="ESP32-C3",
            device_type="esp32",
            api_key="location-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

        device_id = device.id
        new_location_id = new_location.id

    client = app.test_client()
    response = client.post(
        "/api/readings",
        headers={"X-API-Key": "location-api-key"},
        json=[{"sensor_type": "temperature", "value": 24.1, "unit": "C", "location": "GreenHouse"}],
    )

    assert response.status_code == 201

    with app.app_context():
        device = db.session.get(Device, device_id)
        reading = db.session.execute(db.text("SELECT location_id FROM readings")).first()

    assert reading is not None
    assert reading.location_id == new_location_id
    assert device is not None
    assert device.location_id == new_location_id


def test_create_reading_with_new_location_preserves_input_casing():
    with app.app_context():
        user = User(username="new-location-user", email="new-location@example.com", password_hash="hash")
        db.session.add(user)
        db.session.flush()

        device = Device(
            user_id=user.id,
            location_id=None,
            name="ESP32-C3",
            device_type="esp32",
            api_key="new-location-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

        device_id = device.id

    client = app.test_client()
    response = client.post(
        "/api/readings",
        headers={"X-API-Key": "new-location-api-key"},
        json=[{"sensor_type": "humidity", "value": 48.3, "unit": "%", "location": "LIVING_ROOM"}],
    )

    assert response.status_code == 201

    with app.app_context():
        device = db.session.get(Device, device_id)
        created_location = Location.query.filter_by(user_id=device.user_id, name="LIVING_ROOM").first()

    assert created_location is not None
    assert device is not None
    assert device.location_id == created_location.id


def test_create_reading_rejects_non_string_location():
    with app.app_context():
        user = User(username="invalid-location-user", email="invalid-location@example.com", password_hash="hash")
        db.session.add(user)
        db.session.flush()

        device = Device(
            user_id=user.id,
            location_id=None,
            name="ESP32-C3",
            device_type="esp32",
            api_key="invalid-location-api-key",
            is_active=True,
        )
        db.session.add(device)
        db.session.commit()

    client = app.test_client()
    response = client.post(
        "/api/readings",
        headers={"X-API-Key": "invalid-location-api-key"},
        json=[{"sensor_type": "temperature", "value": 21.0, "unit": "C", "location": 123}],
    )

    assert response.status_code == 400
    assert response.get_json()["error"] == "location must be a string"
