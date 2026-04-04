from flask import Blueprint, current_app, jsonify, request, session
from sqlalchemy.exc import IntegrityError
from werkzeug.security import check_password_hash, generate_password_hash

from ..decorators.auth import login_required
from ..extensions import db
from ..models import PasswordResetToken, User
from ..services.auth_service import create_reset_token
from ..services.mail_service import send_reset_password_email
from ..services.rate_limit_service import check_rate_limit, remote_identity
from ..utils.datetime_utils import utcnow_naive
from ..utils.token_utils import hash_reset_token


auth_bp = Blueprint("auth", __name__, url_prefix="/api/auth")


@auth_bp.post("/register")
def auth_register():
    if not current_app.config.get("SELF_REGISTRATION_ENABLED", True):
        return jsonify({"error": "Account creation is currently disabled"}), 403

    payload = request.get_json(silent=True) or {}
    username = (payload.get("username") or "").strip()
    email = (payload.get("email") or "").strip().lower()
    password = payload.get("password") or ""

    if not username or not email or not password:
        return jsonify({"error": "username, email and password are required"}), 400

    if "@" not in email:
        return jsonify({"error": "email is not valid"}), 400

    existing_user = User.query.filter(
        (User.username == username) | (User.email == email)
    ).first()
    if existing_user:
        return jsonify({"error": "username or email already exists"}), 409

    user = User(
        username=username,
        email=email,
        password_hash=generate_password_hash(password),
    )
    db.session.add(user)

    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "username or email already exists"}), 409

    session.clear()
    session.permanent = True
    session["user_id"] = user.id

    return jsonify(user.to_dict()), 201


@auth_bp.post("/login")
def auth_login():
    payload = request.get_json(silent=True) or {}
    username = (payload.get("username") or "").strip()
    password = payload.get("password") or ""

    if not username or not password:
        return jsonify({"error": "username and password are required"}), 400

    user = User.query.filter_by(username=username).first()
    if not user or not check_password_hash(user.password_hash, password):
        return jsonify({"error": "Invalid credentials"}), 401

    session.clear()
    session.permanent = True
    session["user_id"] = user.id

    return jsonify({"message": "Login successful", "user": user.to_dict()}), 200


@auth_bp.post("/forgot-password")
def auth_forgot_password():
    retry_after = check_rate_limit(
        action="forgot-password",
        identity=remote_identity(),
        limit_count=current_app.config["FORGOT_PASSWORD_RATE_LIMIT_COUNT"],
        window_seconds=current_app.config["FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"],
    )
    if retry_after:
        return (
            jsonify(
                {
                    "error": "Too many forgot-password requests. Please try again later.",
                    "retry_after_seconds": retry_after,
                }
            ),
            429,
        )

    payload = request.get_json(silent=True) or {}
    email = (payload.get("email") or "").strip().lower()

    if not email:
        return jsonify({"error": "email is required"}), 400

    user = User.query.filter_by(email=email).first()
    reset_token = None
    email_sent = False
    if user:
        reset_token = create_reset_token(user)
        email_sent = send_reset_password_email(user.email, reset_token)

    response = {"message": "If that email exists, a reset link has been generated"}
    if user and current_app.config["EXPOSE_RESET_TOKEN_IN_RESPONSE"]:
        response["reset_token"] = reset_token
        response["email_sent"] = email_sent

    return jsonify(response), 200


@auth_bp.post("/reset-password")
def auth_reset_password():
    retry_after = check_rate_limit(
        action="reset-password",
        identity=remote_identity(),
        limit_count=current_app.config["RESET_PASSWORD_RATE_LIMIT_COUNT"],
        window_seconds=current_app.config["RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"],
    )
    if retry_after:
        return (
            jsonify(
                {
                    "error": "Too many reset-password requests. Please try again later.",
                    "retry_after_seconds": retry_after,
                }
            ),
            429,
        )

    payload = request.get_json(silent=True) or {}
    token = (payload.get("token") or "").strip()
    password = payload.get("password") or ""

    if not token or not password:
        return jsonify({"error": "token and password are required"}), 400

    token_hash = hash_reset_token(token)
    now = utcnow_naive()
    reset_token = PasswordResetToken.query.filter_by(token_hash=token_hash, used_at=None).first()
    if not reset_token or reset_token.expires_at <= now:
        return jsonify({"error": "Invalid or expired token"}), 400

    user = db.session.get(User, reset_token.user_id)
    if not user:
        return jsonify({"error": "Invalid or expired token"}), 400

    user.password_hash = generate_password_hash(password)
    reset_token.used_at = now
    db.session.commit()

    return jsonify({"message": "Password updated"}), 200


@auth_bp.post("/logout")
def auth_logout():
    session.clear()
    return "", 204


@auth_bp.get("/me")
@login_required
def auth_me(current_user: User):
    return jsonify(current_user.to_dict()), 200


@auth_bp.post("/change-password")
@login_required
def auth_change_password(current_user: User):
    payload = request.get_json(silent=True) or {}
    current_password = payload.get("current_password") or ""
    new_password = payload.get("new_password") or ""

    if not current_password or not new_password:
        return jsonify({"error": "current_password and new_password are required"}), 400

    if not check_password_hash(current_user.password_hash, current_password):
        return jsonify({"error": "Current password is incorrect"}), 401

    current_user.password_hash = generate_password_hash(new_password)
    db.session.commit()

    return jsonify({"message": "Password changed"}), 200
