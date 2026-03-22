import os
import sys
from pathlib import Path

# Ensure tests never depend on external PostgreSQL.
os.environ["DATABASE_URL"] = "sqlite:///:memory:"
os.environ["SECRET_KEY"] = "test-secret"
os.environ["SESSION_COOKIE_SECURE"] = "false"
os.environ["EXPOSE_RESET_TOKEN_IN_RESPONSE"] = "true"

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import run as app_module  # noqa: E402


app = app_module.app
db = app_module.db
User = app_module.User


def _register(client, username: str, email: str, password: str = "secret123"):
    return client.post(
        "/api/auth/register",
        json={"username": username, "email": email, "password": password},
    )


def setup_function(_function):
    with app.app_context():
        db.drop_all()
        db.create_all()


def _set_admin(user_id: int, value: bool = True):
    with app.app_context():
        user = db.session.get(User, user_id)
        user.is_admin = value
        db.session.commit()


def test_users_list_returns_current_user_only():
    client = app.test_client()

    register = _register(client, "alice", "alice@example.com")
    user_id = register.get_json()["id"]

    response = client.get("/api/users")
    assert response.status_code == 200

    payload = response.get_json()
    assert len(payload) == 1
    assert payload[0]["id"] == user_id
    assert payload[0]["username"] == "alice"
    assert payload[0]["is_admin"] is False


def test_admin_can_list_all_users():
    admin_client = app.test_client()
    user_client = app.test_client()

    admin_register = _register(admin_client, "admin", "admin@example.com")
    admin_id = admin_register.get_json()["id"]
    _set_admin(admin_id)

    _register(user_client, "bob", "bob@example.com")

    response = admin_client.get("/api/users")
    assert response.status_code == 200
    payload = response.get_json()
    assert len(payload) == 2
    assert payload[0]["username"] == "admin"
    assert payload[0]["is_admin"] is True


def test_admin_can_create_user_and_issue_reset_token():
    admin_client = app.test_client()

    admin_register = _register(admin_client, "admin", "admin@example.com")
    admin_id = admin_register.get_json()["id"]
    _set_admin(admin_id)

    create = admin_client.post(
        "/api/users",
        json={"username": "new-user", "email": "new-user@example.com", "is_admin": True},
    )
    assert create.status_code == 201
    payload = create.get_json()
    assert payload["message"] == "User created and password setup link generated"
    assert payload["user"]["username"] == "new-user"
    assert payload["user"]["is_admin"] is True
    assert "reset_token" in payload

    login_before_reset = admin_client.post(
        "/api/auth/login",
        json={"username": "new-user", "password": "some-password"},
    )
    assert login_before_reset.status_code == 401

    reset = admin_client.post(
        "/api/auth/reset-password",
        json={"token": payload["reset_token"], "password": "new-user-password"},
    )
    assert reset.status_code == 200

    login_after_reset = admin_client.post(
        "/api/auth/login",
        json={"username": "new-user", "password": "new-user-password"},
    )
    assert login_after_reset.status_code == 200


def test_non_admin_cannot_create_user():
    client = app.test_client()

    _register(client, "alice", "alice@example.com")

    create = client.post(
        "/api/users",
        json={"username": "new-user", "email": "new-user@example.com"},
    )
    assert create.status_code == 403
    assert create.get_json()["error"] == "Only admin can create users"


def test_users_get_forbidden_for_other_user():
    first_client = app.test_client()
    second_client = app.test_client()

    _register(first_client, "alice", "alice@example.com")
    second_register = _register(second_client, "bob", "bob@example.com")
    bob_id = second_register.get_json()["id"]

    forbidden = first_client.get(f"/api/users/{bob_id}")
    assert forbidden.status_code == 403
    assert forbidden.get_json()["error"] == "Forbidden"


def test_users_update_changes_profile_fields():
    client = app.test_client()

    register = _register(client, "alice", "alice@example.com")
    user_id = register.get_json()["id"]

    update = client.patch(
        f"/api/users/{user_id}",
        json={"username": "alice-updated", "email": "newalice@example.com"},
    )
    assert update.status_code == 200
    payload = update.get_json()
    assert payload["username"] == "alice-updated"
    assert payload["email"] == "newalice@example.com"


def test_users_update_rejects_duplicate_email():
    first_client = app.test_client()
    second_client = app.test_client()

    _register(first_client, "alice", "alice@example.com")
    second_register = _register(second_client, "bob", "bob@example.com")
    bob_id = second_register.get_json()["id"]

    duplicate = second_client.patch(
        f"/api/users/{bob_id}",
        json={"email": "alice@example.com"},
    )
    assert duplicate.status_code == 409
    assert duplicate.get_json()["error"] == "email already exists"


def test_non_admin_cannot_change_admin_flag():
    client = app.test_client()

    register = _register(client, "alice", "alice@example.com")
    user_id = register.get_json()["id"]

    forbidden = client.patch(f"/api/users/{user_id}", json={"is_admin": True})
    assert forbidden.status_code == 403
    assert forbidden.get_json()["error"] == "Only admin can change admin privileges"


def test_admin_can_update_other_user_and_reset_password():
    admin_client = app.test_client()
    user_client = app.test_client()

    admin_register = _register(admin_client, "admin", "admin@example.com")
    admin_id = admin_register.get_json()["id"]
    _set_admin(admin_id)

    user_register = _register(user_client, "bob", "bob@example.com")
    bob_id = user_register.get_json()["id"]

    update = admin_client.patch(
        f"/api/users/{bob_id}",
        json={"is_admin": True, "password": "new-bob-password"},
    )
    assert update.status_code == 200
    payload = update.get_json()
    assert payload["is_admin"] is True

    user_client.post("/api/auth/logout")
    old_login = user_client.post(
        "/api/auth/login",
        json={"username": "bob", "password": "secret123"},
    )
    assert old_login.status_code == 401

    new_login = user_client.post(
        "/api/auth/login",
        json={"username": "bob", "password": "new-bob-password"},
    )
    assert new_login.status_code == 200


def test_change_password_requires_current_password_and_updates_login():
    client = app.test_client()

    _register(client, "alice", "alice@example.com")

    wrong_current = client.post(
        "/api/auth/change-password",
        json={"current_password": "wrong", "new_password": "fresh-secret"},
    )
    assert wrong_current.status_code == 401

    changed = client.post(
        "/api/auth/change-password",
        json={"current_password": "secret123", "new_password": "fresh-secret"},
    )
    assert changed.status_code == 200

    client.post("/api/auth/logout")
    old_login = client.post(
        "/api/auth/login",
        json={"username": "alice", "password": "secret123"},
    )
    assert old_login.status_code == 401

    new_login = client.post(
        "/api/auth/login",
        json={"username": "alice", "password": "fresh-secret"},
    )
    assert new_login.status_code == 200


def test_users_delete_removes_account_and_session():
    client = app.test_client()

    register = _register(client, "alice", "alice@example.com")
    user_id = register.get_json()["id"]

    delete = client.delete(f"/api/users/{user_id}")
    assert delete.status_code == 204

    me = client.get("/api/auth/me")
    assert me.status_code == 401
