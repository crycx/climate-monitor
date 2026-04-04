import os
import sys
from pathlib import Path

# Ensure tests never depend on external PostgreSQL.
os.environ["DATABASE_URL"] = "sqlite:///:memory:"
os.environ["SECRET_KEY"] = "test-secret"
os.environ["SESSION_COOKIE_SECURE"] = "false"
os.environ["EXPOSE_RESET_TOKEN_IN_RESPONSE"] = "true"
os.environ["FORGOT_PASSWORD_RATE_LIMIT_COUNT"] = "2"
os.environ["FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"] = "60"

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import run as app_module  # noqa: E402


app = app_module.app
db = app_module.db


def _register(client, username: str = "tester", email: str = "tester@example.com"):
    return client.post(
        "/api/auth/register",
        json={"username": username, "email": email, "password": "secret123"},
    )


def _login(client, username: str = "tester", password: str = "secret123"):
    return client.post(
        "/api/auth/login",
        json={"username": username, "password": password},
    )


def setup_function(_function):
    with app.app_context():
        db.drop_all()
        db.create_all()

    # Keep tests isolated from in-memory rate limiter state.
    app_module._reset_rate_limit_store_for_tests()


def test_register_sets_session_and_returns_user():
    client = app.test_client()

    response = _register(client)

    assert response.status_code == 201
    payload = response.get_json()
    assert payload["username"] == "tester"
    assert payload["email"] == "tester@example.com"

    me = client.get("/api/auth/me")
    assert me.status_code == 200
    assert me.get_json()["username"] == "tester"


def test_logout_invalidates_session():
    client = app.test_client()

    _register(client)
    logout = client.post("/api/auth/logout")
    assert logout.status_code == 204

    me = client.get("/api/auth/me")
    assert me.status_code == 401


def test_login_with_valid_credentials_restores_session():
    client = app.test_client()

    _register(client)
    client.post("/api/auth/logout")

    login = _login(client)
    assert login.status_code == 200
    payload = login.get_json()
    assert payload["message"] == "Login successful"
    assert payload["user"]["username"] == "tester"

    me = client.get("/api/auth/me")
    assert me.status_code == 200


def test_login_rejects_invalid_password():
    client = app.test_client()

    _register(client)
    client.post("/api/auth/logout")

    login = _login(client, password="wrong-password")
    assert login.status_code == 401
    assert login.get_json()["error"] == "Invalid credentials"


def test_forgot_and_reset_password_flow():
    client = app.test_client()

    _register(client, username="reset-user", email="reset@example.com")
    client.post("/api/auth/logout")

    forgot = client.post("/api/auth/forgot-password", json={"email": "reset@example.com"})
    assert forgot.status_code == 200
    forgot_payload = forgot.get_json()
    assert "reset_token" in forgot_payload

    reset = client.post(
        "/api/auth/reset-password",
        json={"token": forgot_payload["reset_token"], "password": "new-secret-123"},
    )
    assert reset.status_code == 200

    old_login = _login(client, username="reset-user", password="secret123")
    assert old_login.status_code == 401

    new_login = _login(client, username="reset-user", password="new-secret-123")
    assert new_login.status_code == 200


def test_forgot_password_rate_limited_after_threshold():
    client = app.test_client()

    _register(client, username="rl-user", email="rl@example.com")
    client.post("/api/auth/logout")

    first = client.post("/api/auth/forgot-password", json={"email": "rl@example.com"})
    second = client.post("/api/auth/forgot-password", json={"email": "rl@example.com"})
    third = client.post("/api/auth/forgot-password", json={"email": "rl@example.com"})

    assert first.status_code == 200
    assert second.status_code == 200
    assert third.status_code == 429
    payload = third.get_json()
    assert payload["error"].startswith("Too many forgot-password requests")
    assert payload["retry_after_seconds"] >= 1
