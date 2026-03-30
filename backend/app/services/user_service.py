from flask import Response, jsonify
from sqlalchemy import inspect, text
from werkzeug.security import generate_password_hash

from ..extensions import db
from ..utils.env_utils import to_bool


def ensure_users_is_admin_column() -> None:
    columns = {column["name"] for column in inspect(db.engine).get_columns("users")}
    if "is_admin" in columns:
        return

    db.session.execute(text("ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT FALSE"))
    db.session.commit()


def require_manage_access(current_user, user_id: int):
    if not current_user.is_admin and current_user.id != user_id:
        return jsonify({"error": "Forbidden"}), 403
    return None


def validate_user_update_payload(
    payload: dict,
    *,
    actor_is_admin: bool,
) -> tuple[dict, tuple[Response, int] | None]:
    username = payload.get("username")
    email = payload.get("email")
    password = payload.get("password")
    is_admin = payload.get("is_admin")

    updates: dict = {}

    if username is not None:
        cleaned_username = str(username).strip()
        if not cleaned_username:
            return {}, (jsonify({"error": "username cannot be empty"}), 400)
        updates["username"] = cleaned_username

    if email is not None:
        cleaned_email = str(email).strip().lower()
        if not cleaned_email or "@" not in cleaned_email:
            return {}, (jsonify({"error": "email is not valid"}), 400)
        updates["email"] = cleaned_email

    if password is not None:
        if not actor_is_admin:
            return {}, (
                jsonify({"error": "Only admin can reset password via this endpoint"}),
                403,
            )
        cleaned_password = str(password)
        if not cleaned_password:
            return {}, (jsonify({"error": "password cannot be empty"}), 400)
        updates["password_hash"] = generate_password_hash(cleaned_password)

    if is_admin is not None:
        if not actor_is_admin:
            return {}, (jsonify({"error": "Only admin can change admin privileges"}), 403)

        if isinstance(is_admin, bool):
            updates["is_admin"] = is_admin
        elif isinstance(is_admin, str):
            updates["is_admin"] = to_bool(is_admin)
        else:
            return {}, (jsonify({"error": "is_admin must be boolean"}), 400)

    if not updates:
        return {}, (
            jsonify({"error": "At least one of username, email, or password is required"}),
            400,
        )

    return updates, None


def validate_user_create_payload(
    payload: dict,
    *,
    actor_is_admin: bool,
) -> tuple[dict, tuple[Response, int] | None]:
    if not actor_is_admin:
        return {}, (jsonify({"error": "Only admin can create users"}), 403)

    username = (payload.get("username") or "").strip()
    email = (payload.get("email") or "").strip().lower()
    is_admin = payload.get("is_admin", False)

    if not username or not email:
        return {}, (jsonify({"error": "username and email are required"}), 400)

    if "@" not in email:
        return {}, (jsonify({"error": "email is not valid"}), 400)

    if isinstance(is_admin, bool):
        normalized_is_admin = is_admin
    elif isinstance(is_admin, str):
        normalized_is_admin = to_bool(is_admin)
    else:
        return {}, (jsonify({"error": "is_admin must be boolean"}), 400)

    return {
        "username": username,
        "email": email,
        "is_admin": normalized_is_admin,
    }, None
