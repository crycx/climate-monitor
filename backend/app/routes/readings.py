from datetime import datetime, timedelta, timezone
import csv
import io
import secrets

from flask import Blueprint, jsonify, make_response, request
from sqlalchemy.exc import IntegrityError
from sqlalchemy import func, or_

from ..decorators.api_key import api_key_required
from ..decorators.auth import login_required
from ..extensions import db
from ..models import Device, DeviceShare, Location, Reading, User


readings_bp = Blueprint("readings", __name__, url_prefix="/api/readings")
devices_bp = Blueprint("devices", __name__, url_prefix="/api/devices")


def _parse_iso_timestamp(value: str, *, parameter_name: str):
    normalized = value.strip()
    if normalized.endswith("Z"):
        normalized = normalized[:-1] + "+00:00"

    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError:
        return None, (jsonify({"error": f"Invalid {parameter_name}; expected ISO-8601 timestamp"}), 400)

    if parsed.tzinfo is not None:
        parsed = parsed.astimezone(timezone.utc).replace(tzinfo=None)

    return parsed, None


def _now_utc_naive() -> datetime:
    return datetime.now(timezone.utc).replace(tzinfo=None)


def _period_window(period: str):
    utc_now = datetime.now(timezone.utc)
    today_start = datetime(
        utc_now.year,
        utc_now.month,
        utc_now.day,
    )

    period_value = period.strip().lower()
    if period_value == "today":
        return today_start, None
    if period_value == "yesterday":
        start = today_start - timedelta(days=1)
        end = today_start
        return start, end
    if period_value == "last_24h":
        start = (utc_now - timedelta(hours=24)).replace(tzinfo=None)
        return start, None

    return None, (jsonify({"error": "Invalid period; use today, yesterday, or last_24h"}), 400)


def _apply_readings_filters(query):
    limit = request.args.get("limit", 50000, type=int)
    if limit is None or limit < 1:
        return None, None, (jsonify({"error": "limit must be a positive integer"}), 400)
    clamped_limit = min(limit, 50000)

    order = (request.args.get("order") or "desc").strip().lower()
    if order not in {"asc", "desc"}:
        return None, None, (jsonify({"error": "order must be asc or desc"}), 400)

    location_id = request.args.get("location_id", type=int)
    sensor_type = request.args.get("sensor_type")
    after_id = request.args.get("after_id", type=int)
    after_ts_raw = request.args.get("after_ts")
    from_ts_raw = request.args.get("from_ts")
    to_ts_raw = request.args.get("to_ts")
    period = request.args.get("period")

    if period and (from_ts_raw or to_ts_raw):
        return None, None, (jsonify({"error": "period cannot be combined with from_ts/to_ts"}), 400)

    if location_id is not None:
        query = query.filter(Reading.location_id == location_id)
    if sensor_type:
        query = query.filter(Reading.sensor_type == sensor_type)
    if after_id is not None:
        query = query.filter(Reading.id > after_id)

    if after_ts_raw:
        after_ts, error = _parse_iso_timestamp(after_ts_raw, parameter_name="after_ts")
        if error:
            return None, None, error
        query = query.filter(Reading.timestamp > after_ts)

    if period:
        start, end_or_error = _period_window(period)
        if isinstance(end_or_error, tuple):
            return None, None, end_or_error
        end = end_or_error
        query = query.filter(Reading.timestamp >= start)
        if end is not None:
            query = query.filter(Reading.timestamp < end)
    else:
        if from_ts_raw:
            from_ts, error = _parse_iso_timestamp(from_ts_raw, parameter_name="from_ts")
            if error:
                return None, None, error
            query = query.filter(Reading.timestamp >= from_ts)

        if to_ts_raw:
            to_ts, error = _parse_iso_timestamp(to_ts_raw, parameter_name="to_ts")
            if error:
                return None, None, error
            query = query.filter(Reading.timestamp <= to_ts)

    ordering = Reading.timestamp.asc() if order == "asc" else Reading.timestamp.desc()
    query = query.order_by(ordering)

    return query, clamped_limit, None


def _parse_readings_payload(payload, *, expect_array: bool):
    if expect_array:
        if not payload or not isinstance(payload, list):
            return None, (jsonify({"error": "Expected a JSON array of readings"}), 400)
        items = payload
    else:
        if not payload or not isinstance(payload, dict):
            return None, (jsonify({"error": "Expected a JSON object reading"}), 400)
        items = [payload]

    normalized_items = []
    for item in items:
        sensor_type = item.get("sensor_type")
        value = item.get("value")
        unit = item.get("unit")
        location_name_raw = item.get("location")

        if not sensor_type or value is None or not unit:
            return None, (jsonify({"error": "Each reading must have sensor_type, value and unit"}), 400)

        if location_name_raw is not None and not isinstance(location_name_raw, str):
            return None, (jsonify({"error": "location must be a string"}), 400)

        location_name = None
        location_name_normalized = None
        if isinstance(location_name_raw, str):
            location_name = location_name_raw.strip()
            if location_name:
                location_name_normalized = location_name.lower()
            else:
                location_name = None

        normalized_items.append({
            "sensor_type": sensor_type,
            "value": value,
            "unit": unit,
            "location_name": location_name,
            "location_name_normalized": location_name_normalized,
        })

    return normalized_items, None


def _resolve_location_for_device(
    device: Device,
    location_name: str | None,
    location_name_normalized: str | None,
):
    if not location_name or not location_name_normalized:
        return None

    location = (
        Location.query
        .filter(Location.user_id == device.user_id)
        .filter(func.lower(Location.name) == location_name_normalized)
        .first()
    )
    if location:
        return location

    now = datetime.now(timezone.utc)
    location = Location(
        user_id=device.user_id,
        name=location_name,
        created_at=now,
        updated_at=now,
    )
    db.session.add(location)
    db.session.flush()
    return location


def _save_readings(device: Device, readings_payload: list[dict]) -> int:
    readings = []
    latest_location_id = device.location_id
    for item in readings_payload:
        location = _resolve_location_for_device(
            device,
            item["location_name"],
            item["location_name_normalized"],
        )
        location_id = location.id if location else device.location_id

        if location_id is not None:
            latest_location_id = location_id

        readings.append(Reading(
            device_id=device.id,
            location_id=location_id,
            sensor_type=item["sensor_type"],
            value=item["value"],
            unit=item["unit"],
            timestamp=datetime.now(timezone.utc),
        ))

    if latest_location_id != device.location_id:
        device.location_id = latest_location_id
        device.updated_at = datetime.now(timezone.utc)

    db.session.add_all(readings)
    db.session.commit()
    return len(readings)


def _can_access_device(current_user: User, device: Device) -> bool:
    if current_user.is_admin or device.user_id == current_user.id:
        return True

    now = _now_utc_naive()
    shared = (
        DeviceShare.query
        .filter(DeviceShare.device_id == device.id)
        .filter(DeviceShare.shared_with_user_id == current_user.id)
        .filter(DeviceShare.starts_at <= now)
        .filter(or_(DeviceShare.ends_at.is_(None), DeviceShare.ends_at >= now))
        .first()
    )
    return shared is not None


def _can_manage_device(current_user: User, device: Device) -> bool:
    return current_user.is_admin or device.user_id == current_user.id


def _accessible_device_ids(current_user: User) -> set[int] | None:
    if current_user.is_admin:
        return None

    own_ids = {
        row[0]
        for row in db.session.query(Device.id)
        .filter(Device.user_id == current_user.id)
        .all()
    }

    now = _now_utc_naive()
    shared_ids = {
        row[0]
        for row in db.session.query(DeviceShare.device_id)
        .filter(DeviceShare.shared_with_user_id == current_user.id)
        .filter(DeviceShare.starts_at <= now)
        .filter(or_(DeviceShare.ends_at.is_(None), DeviceShare.ends_at >= now))
        .all()
    }

    return own_ids | shared_ids


def _share_payload(share: DeviceShare) -> dict:
    shared_user = db.session.get(User, share.shared_with_user_id)
    payload = share.to_dict()
    payload["shared_with"] = {
        "id": shared_user.id,
        "username": shared_user.username,
        "email": shared_user.email,
    } if shared_user else None
    return payload


def _format_csv_number(value) -> str:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return ""

    formatted = f"{number:.4f}".rstrip("0").rstrip(".")
    return formatted if formatted else "0"


def _format_csv_timestamp(value: datetime) -> str:
    return value.strftime("%d.%m.%Y %H:%M:%S")


def _resolve_location_for_user(current_user: User, location_id: int | None):
    if location_id is None:
        return None, None

    location = db.session.get(Location, location_id)
    if not location:
        return None, (jsonify({"error": "Location not found"}), 404)

    if not current_user.is_admin and location.user_id != current_user.id:
        return None, (jsonify({"error": "Access denied"}), 403)

    return location, None


@readings_bp.post("")
@api_key_required
def create_reading(device: Device):
    """
    POST /api/readings
    Header: X-API-Key: <device api key>
    Body: [{"sensor_type": "temperature", "value": 23.5, "unit": "C"}, ...]
    """
    payload = request.get_json(silent=True)
    readings_payload, error = _parse_readings_payload(payload, expect_array=True)
    if error:
        return error

    saved_count = _save_readings(device, readings_payload)
    return jsonify({"saved": saved_count}), 201


@devices_bp.post("/<int:device_id>/readings")
@api_key_required
def create_device_reading(device: Device, device_id: int):
    """
    Legacy compatibility endpoint for payloads that post to /api/devices/<id>/readings.
    Header: X-API-Key: <device api key>
    Body: {"sensor_type": "temperature", "value": 23.5, "unit": "C", "location": "greenhouse"}
    """
    if device.id != device_id:
        return jsonify({"error": "Device ID does not match API key"}), 403

    payload = request.get_json(silent=True)
    readings_payload, error = _parse_readings_payload(payload, expect_array=False)
    if error:
        return error

    saved_count = _save_readings(device, readings_payload)
    return jsonify({"saved": saved_count}), 201


@devices_bp.get("")
@login_required
def list_devices(current_user: User):
    if current_user.is_admin:
        devices = Device.query.order_by(Device.id.asc()).all()
    else:
        accessible_ids = _accessible_device_ids(current_user) or set()
        if not accessible_ids:
            return jsonify([]), 200
        devices = Device.query.filter(Device.id.in_(accessible_ids)).order_by(Device.id.asc()).all()
    return jsonify([d.to_dict() for d in devices]), 200


@devices_bp.post("")
@login_required
def create_device(current_user: User):
    payload = request.get_json(silent=True) or {}

    name = (payload.get("name") or "").strip()
    if not name:
        return jsonify({"error": "name is required"}), 400

    user_id = payload.get("user_id", current_user.id) if current_user.is_admin else current_user.id
    if not isinstance(user_id, int) or not db.session.get(User, user_id):
        return jsonify({"error": "Invalid user_id"}), 400

    location_id = payload.get("location_id")
    if location_id is not None and not isinstance(location_id, int):
        return jsonify({"error": "location_id must be an integer"}), 400

    location, location_error = _resolve_location_for_user(current_user, location_id)
    if location_error:
        return location_error
    if location and location.user_id not in {None, user_id}:
        return jsonify({"error": "Location does not belong to selected user"}), 400

    api_key = (payload.get("api_key") or "").strip() or secrets.token_hex(32)
    if len(api_key) != 64:
        return jsonify({"error": "api_key must be 64 characters"}), 400

    now = datetime.now(timezone.utc)
    device = Device(
        user_id=user_id,
        location_id=location_id,
        name=name,
        device_type=(payload.get("device_type") or None),
        api_key=api_key,
        is_active=bool(payload.get("is_active", True)),
        created_at=now,
        updated_at=now,
    )

    db.session.add(device)
    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "api_key already exists"}), 409

    result = device.to_dict()
    result["api_key"] = device.api_key
    return jsonify(result), 201


@devices_bp.get("/<int:device_id>")
@login_required
def get_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    result = device.to_dict()
    result["api_key"] = device.api_key
    result["can_manage"] = _can_manage_device(current_user, device)
    return jsonify(result), 200


@devices_bp.patch("/<int:device_id>")
@login_required
def update_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    payload = request.get_json(silent=True) or {}

    if "name" in payload:
        name = (payload.get("name") or "").strip()
        if not name:
            return jsonify({"error": "name cannot be empty"}), 400
        device.name = name

    if "device_type" in payload:
        device.device_type = payload.get("device_type") or None

    if "location_id" in payload:
        location_id = payload.get("location_id")
        if location_id is not None and not isinstance(location_id, int):
            return jsonify({"error": "location_id must be an integer"}), 400
        location, location_error = _resolve_location_for_user(current_user, location_id)
        if location_error:
            return location_error
        if location and location.user_id not in {None, device.user_id}:
            return jsonify({"error": "Location does not belong to device owner"}), 400
        device.location_id = location_id

    if "is_active" in payload:
        device.is_active = bool(payload.get("is_active"))

    if "api_key" in payload:
        api_key = (payload.get("api_key") or "").strip()
        if len(api_key) != 64:
            return jsonify({"error": "api_key must be 64 characters"}), 400
        device.api_key = api_key

    device.updated_at = datetime.now(timezone.utc)

    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "api_key already exists"}), 409

    result = device.to_dict()
    result["api_key"] = device.api_key
    return jsonify(result), 200


@devices_bp.delete("/<int:device_id>")
@login_required
def delete_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    db.session.delete(device)
    db.session.commit()
    return "", 204


@devices_bp.get("/<int:device_id>/readings")
@login_required
def device_readings(current_user: User, device_id: int):
    device = Device.query.get_or_404(device_id)
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    query = Reading.query.filter_by(device_id=device_id)
    query, clamped_limit, error = _apply_readings_filters(query)
    if error:
        return error

    readings = query.limit(clamped_limit).all()
    return jsonify([r.to_dict() for r in readings]), 200


@devices_bp.get("/<int:device_id>/readings.csv")
@login_required
def device_readings_csv(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    from_ts_raw = request.args.get("from_ts")
    to_ts_raw = request.args.get("to_ts")

    query = Reading.query.filter(Reading.device_id == device_id)

    if from_ts_raw:
        from_ts, error = _parse_iso_timestamp(from_ts_raw, parameter_name="from_ts")
        if error:
            return error
        query = query.filter(Reading.timestamp >= from_ts)

    if to_ts_raw:
        to_ts, error = _parse_iso_timestamp(to_ts_raw, parameter_name="to_ts")
        if error:
            return error
        query = query.filter(Reading.timestamp <= to_ts)

    readings = query.order_by(Reading.timestamp.asc(), Reading.id.asc()).all()

    rows_by_ts: dict[int, dict[str, str]] = {}
    for reading in readings:
        if reading.timestamp is None:
            continue
        if reading.sensor_type not in {"temperature", "humidity"}:
            continue

        ts = reading.timestamp
        if ts.tzinfo is None:
            ts = ts.replace(tzinfo=timezone.utc)
        else:
            ts = ts.astimezone(timezone.utc)

        # Group by second to align temperature/humidity values captured in the same sample window.
        ts_sec = ts.replace(microsecond=0)
        key = int(ts_sec.timestamp())
        row = rows_by_ts.setdefault(
            key,
            {
                "timestamp": _format_csv_timestamp(ts_sec),
                "temperature": "",
                "humidity": "",
            },
        )
        row[reading.sensor_type] = _format_csv_number(reading.value)

    output = io.StringIO()
    writer = csv.DictWriter(output, fieldnames=["timestamp", "temperature", "humidity"])
    writer.writeheader()
    for timestamp_key in sorted(rows_by_ts.keys()):
        writer.writerow(rows_by_ts[timestamp_key])

    response = make_response(output.getvalue())
    response.headers["Content-Type"] = "text/csv; charset=utf-8"
    response.headers["Content-Disposition"] = f'attachment; filename="device-{device_id}-readings.csv"'
    return response


@devices_bp.get("/<int:device_id>/shares")
@login_required
def list_device_shares(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    shares = (
        DeviceShare.query
        .filter(DeviceShare.device_id == device_id)
        .order_by(DeviceShare.starts_at.asc(), DeviceShare.id.asc())
        .all()
    )
    return jsonify([_share_payload(share) for share in shares]), 200


@devices_bp.get("/<int:device_id>/shareable-users")
@login_required
def list_shareable_users(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    users = (
        User.query
        .filter(User.id != device.user_id)
        .order_by(User.username.asc(), User.id.asc())
        .all()
    )
    return jsonify([user.to_dict() for user in users]), 200


@devices_bp.post("/<int:device_id>/shares")
@login_required
def create_device_share(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    payload = request.get_json(silent=True) or {}
    shared_user = None
    shared_with_user_id = payload.get("shared_with_user_id")
    email = (payload.get("shared_with_email") or "").strip().lower()

    if shared_with_user_id is not None:
        if not isinstance(shared_with_user_id, int):
            return jsonify({"error": "shared_with_user_id must be an integer"}), 400
        shared_user = db.session.get(User, shared_with_user_id)
        if not shared_user:
            return jsonify({"error": "User not found for shared_with_user_id"}), 404
    else:
        if not email:
            return jsonify({"error": "shared_with_user_id or shared_with_email is required"}), 400
        shared_user = User.query.filter(User.email == email).first()

    if not shared_user:
        return jsonify({"error": "User not found"}), 404
    if shared_user.id == device.user_id:
        return jsonify({"error": "Device owner cannot be added as shared user"}), 400

    starts_at_raw = payload.get("starts_at")
    ends_at_raw = payload.get("ends_at")

    if not isinstance(starts_at_raw, str) or not starts_at_raw.strip():
        return jsonify({"error": "starts_at is required"}), 400
    if not isinstance(ends_at_raw, str) or not ends_at_raw.strip():
        return jsonify({"error": "ends_at is required"}), 400

    starts_at, error = _parse_iso_timestamp(starts_at_raw, parameter_name="starts_at")
    if error:
        return error
    ends_at, error = _parse_iso_timestamp(ends_at_raw, parameter_name="ends_at")
    if error:
        return error
    if starts_at >= ends_at:
        return jsonify({"error": "starts_at must be earlier than ends_at"}), 400

    overlaps = (
        DeviceShare.query
        .filter(DeviceShare.device_id == device.id)
        .filter(DeviceShare.shared_with_user_id == shared_user.id)
        .filter(DeviceShare.starts_at <= ends_at)
        .filter(or_(DeviceShare.ends_at.is_(None), DeviceShare.ends_at >= starts_at))
        .first()
    )
    if overlaps:
        return jsonify({"error": "Overlapping share period already exists for this user"}), 409

    share = DeviceShare(
        device_id=device.id,
        shared_with_user_id=shared_user.id,
        starts_at=starts_at,
        ends_at=ends_at,
        created_at=datetime.now(timezone.utc),
    )
    db.session.add(share)
    db.session.commit()
    return jsonify(_share_payload(share)), 201


@devices_bp.delete("/<int:device_id>/shares/<int:share_id>")
@login_required
def delete_device_share(current_user: User, device_id: int, share_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_manage_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    share = db.session.get(DeviceShare, share_id)
    if not share or share.device_id != device_id:
        return jsonify({"error": "Share not found"}), 404

    db.session.delete(share)
    db.session.commit()
    return "", 204


@readings_bp.get("/latest")
@login_required
def latest_readings(current_user: User):
    query = Reading.query.join(Device, Reading.device_id == Device.id)
    if not current_user.is_admin:
        accessible_ids = _accessible_device_ids(current_user) or set()
        query = query.filter(Device.id.in_(accessible_ids))

    query, clamped_limit, error = _apply_readings_filters(query)
    if error:
        return error

    readings = query.limit(clamped_limit).all()
    return jsonify([r.to_dict() for r in readings]), 200


@readings_bp.get("/latest_device")
@api_key_required
def latest_device_reading(device: Device):
    """
    GET /api/readings/latest_device
    Header: X-API-Key: <device api key>
    Optional query: sensor_type=temperature

    Returns the latest reading for the calling device.
    """
    sensor_type = request.args.get("sensor_type")
    query = Reading.query.filter_by(device_id=device.id)
    if sensor_type:
        query = query.filter(Reading.sensor_type == sensor_type)

    reading = query.order_by(Reading.timestamp.desc(), Reading.id.desc()).first()
    if not reading:
        return jsonify({"error": "No readings found"}), 404

    return jsonify(reading.to_dict()), 200
