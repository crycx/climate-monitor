import secrets

from flask import Blueprint, current_app, jsonify, request, session
from sqlalchemy.exc import IntegrityError
from werkzeug.security import generate_password_hash

from ..decorators.auth import login_required
from ..extensions import db
from ..models import User
from ..services.auth_service import create_reset_token
from ..services.mail_service import send_reset_password_email
from ..services.user_service import (
    require_manage_access,
    validate_user_create_payload,
    validate_user_update_payload,
)


users_bp = Blueprint("users", __name__, url_prefix="/api/users")


@users_bp.get("")
@login_required
def users_list(current_user: User):
    if current_user.is_admin:
        users = User.query.order_by(User.id.asc()).all()
        return jsonify([user.to_dict() for user in users]), 200

    return jsonify([current_user.to_dict()]), 200


@users_bp.post("")
@login_required
def users_create(current_user: User):
    payload = request.get_json(silent=True) or {}
    user_data, validation_error = validate_user_create_payload(
        payload,
        actor_is_admin=current_user.is_admin,
    )
    if validation_error:
        return validation_error

    existing_user = User.query.filter(
        (User.username == user_data["username"]) | (User.email == user_data["email"])
    ).first()
    if existing_user:
        return jsonify({"error": "username or email already exists"}), 409

    user = User(
        username=user_data["username"],
        email=user_data["email"],
        is_admin=user_data["is_admin"],
        password_hash=generate_password_hash(secrets.token_urlsafe(24)),
    )
    db.session.add(user)

    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "username or email already exists"}), 409

    reset_token = create_reset_token(user)
    email_sent = send_reset_password_email(user.email, reset_token)

    response = {
        "message": "User created and password setup link generated",
        "user": user.to_dict(),
        "email_sent": email_sent,
    }
    if current_app.config["EXPOSE_RESET_TOKEN_IN_RESPONSE"]:
        response["reset_token"] = reset_token

    return jsonify(response), 201


@users_bp.get("/<int:user_id>")
@login_required
def users_get(current_user: User, user_id: int):
    forbidden = require_manage_access(current_user, user_id)
    if forbidden:
        return forbidden

    user = db.session.get(User, user_id)
    if not user:
        return jsonify({"error": "User not found"}), 404

    return jsonify(user.to_dict()), 200


@users_bp.patch("/<int:user_id>")
@login_required
def users_update(current_user: User, user_id: int):
    forbidden = require_manage_access(current_user, user_id)
    if forbidden:
        return forbidden

    user = db.session.get(User, user_id)
    if not user:
        return jsonify({"error": "User not found"}), 404

    payload = request.get_json(silent=True) or {}
    updates, validation_error = validate_user_update_payload(
        payload,
        actor_is_admin=current_user.is_admin,
    )
    if validation_error:
        return validation_error

    new_username = updates.get("username")
    if new_username and new_username != user.username:
        existing = User.query.filter(User.username == new_username, User.id != user.id).first()
        if existing:
            return jsonify({"error": "username already exists"}), 409

    new_email = updates.get("email")
    if new_email and new_email != user.email:
        existing = User.query.filter(User.email == new_email, User.id != user.id).first()
        if existing:
            return jsonify({"error": "email already exists"}), 409

    for key, value in updates.items():
        setattr(user, key, value)

    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "username or email already exists"}), 409

    return jsonify(user.to_dict()), 200


@users_bp.delete("/<int:user_id>")
@login_required
def users_delete(current_user: User, user_id: int):
    forbidden = require_manage_access(current_user, user_id)
    if forbidden:
        return forbidden

    user = db.session.get(User, user_id)
    if not user:
        return jsonify({"error": "User not found"}), 404

    deleting_self = current_user.id == user.id
    db.session.delete(user)
    db.session.commit()

    if deleting_self:
        session.clear()

    return "", 204
