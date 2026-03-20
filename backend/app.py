import os
import hashlib
import secrets
import smtplib
import time
import threading
from collections import defaultdict, deque
from datetime import UTC, datetime, timedelta
from email.message import EmailMessage
from functools import wraps
from pathlib import Path
from urllib.parse import urlencode

from dotenv import load_dotenv
from flask_cors import CORS
from flask import Flask, Response, jsonify, request, send_file, session
from flask_sqlalchemy import SQLAlchemy
from sqlalchemy.exc import IntegrityError
from werkzeug.security import check_password_hash, generate_password_hash

app = Flask(__name__)
db = SQLAlchemy()


_rate_limit_store: dict[str, deque[float]] = defaultdict(deque)
_rate_limit_lock = threading.Lock()


def _utcnow_naive() -> datetime:
    return datetime.now(UTC).replace(tzinfo=None)


def _to_bool(value: str | None, default: bool = False) -> bool:
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _database_url() -> str:
    configured = os.getenv("DATABASE_URL")
    if configured:
        return configured

    db_name = os.getenv("POSTGRES_DB")
    db_password = os.getenv("POSTGRES_PASSWORD")
    db_user = os.getenv("POSTGRES_USER", "postgres")
    db_host = os.getenv("POSTGRES_HOST", "localhost")
    db_port = os.getenv("POSTGRES_PORT", "5432")

    if db_name and db_password:
        return f"postgresql://{db_user}:{db_password}@{db_host}:{db_port}/{db_name}"

    # Local fallback for development if env vars are not available.
    return "sqlite:///climate_monitor.db"


def _load_env() -> None:
    backend_env = Path(app.root_path) / ".env"
    repo_env = Path(app.root_path).parent / ".env"
    load_dotenv(backend_env, override=False)
    load_dotenv(repo_env, override=False)


def _configure_app() -> None:
    _load_env()

    frontend_origin = os.getenv("FRONTEND_ORIGIN", "http://localhost:4200")

    app.config["SQLALCHEMY_DATABASE_URI"] = _database_url()
    app.config["SQLALCHEMY_TRACK_MODIFICATIONS"] = False
    app.config["SECRET_KEY"] = os.getenv("SECRET_KEY", "dev-only-secret-change-me")
    app.config["SESSION_COOKIE_HTTPONLY"] = True
    app.config["SESSION_COOKIE_SAMESITE"] = os.getenv("SESSION_COOKIE_SAMESITE", "Lax")
    app.config["SESSION_COOKIE_SECURE"] = _to_bool(os.getenv("SESSION_COOKIE_SECURE"), False)
    app.config["PERMANENT_SESSION_LIFETIME"] = timedelta(
        hours=int(os.getenv("SESSION_LIFETIME_HOURS", "24"))
    )
    app.config["RESET_TOKEN_TTL_MINUTES"] = int(os.getenv("RESET_TOKEN_TTL_MINUTES", "30"))
    app.config["EXPOSE_RESET_TOKEN_IN_RESPONSE"] = _to_bool(
        os.getenv("EXPOSE_RESET_TOKEN_IN_RESPONSE"),
        _to_bool(os.getenv("FLASK_DEBUG"), False),
    )
    app.config["FRONTEND_RESET_URL"] = os.getenv(
        "FRONTEND_RESET_URL", "http://localhost:4200/reset-password"
    )
    app.config["SMTP_HOST"] = os.getenv("SMTP_HOST", "")
    app.config["SMTP_PORT"] = int(os.getenv("SMTP_PORT", "587"))
    app.config["SMTP_USERNAME"] = os.getenv("SMTP_USERNAME", "")
    app.config["SMTP_PASSWORD"] = os.getenv("SMTP_PASSWORD", "")
    app.config["SMTP_USE_TLS"] = _to_bool(os.getenv("SMTP_USE_TLS"), True)
    app.config["MAIL_FROM"] = os.getenv("MAIL_FROM", "no-reply@climate-monitor.local")
    app.config["FORGOT_PASSWORD_RATE_LIMIT_COUNT"] = int(
        os.getenv("FORGOT_PASSWORD_RATE_LIMIT_COUNT", "5")
    )
    app.config["FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"] = int(
        os.getenv("FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS", "900")
    )
    app.config["RESET_PASSWORD_RATE_LIMIT_COUNT"] = int(
        os.getenv("RESET_PASSWORD_RATE_LIMIT_COUNT", "10")
    )
    app.config["RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"] = int(
        os.getenv("RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS", "900")
    )

    CORS(
        app,
        supports_credentials=True,
        resources={r"/api/*": {"origins": [frontend_origin]}},
    )

    db.init_app(app)


class User(db.Model):
    __tablename__ = "users"

    id = db.Column(db.Integer, primary_key=True)
    username = db.Column(db.String(100), unique=True, nullable=False)
    email = db.Column(db.String(255), unique=True, nullable=False)
    password_hash = db.Column(db.String(255), nullable=False)
    created_at = db.Column(db.DateTime)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "username": self.username,
            "email": self.email,
            "created_at": self.created_at.isoformat() if self.created_at else None,
        }


class PasswordResetToken(db.Model):
    __tablename__ = "password_reset_tokens"

    id = db.Column(db.Integer, primary_key=True)
    user_id = db.Column(db.Integer, db.ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    token_hash = db.Column(db.String(64), unique=True, nullable=False)
    expires_at = db.Column(db.DateTime, nullable=False)
    used_at = db.Column(db.DateTime, nullable=True)
    created_at = db.Column(db.DateTime, default=_utcnow_naive, nullable=False)


def _hash_reset_token(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def _remote_identity() -> str:
    forwarded_for = request.headers.get("X-Forwarded-For")
    if forwarded_for:
        # Trust the left-most value as client identity when behind a proxy.
        return forwarded_for.split(",")[0].strip()
    return request.remote_addr or "unknown"


def _rate_limit_key(action: str, identity: str) -> str:
    return f"{action}:{identity}"


def _check_rate_limit(action: str, identity: str, limit_count: int, window_seconds: int) -> int:
    """Return 0 when allowed, or number of seconds until next allowed request."""
    now = time.time()
    window_start = now - window_seconds
    key = _rate_limit_key(action, identity)

    with _rate_limit_lock:
        hits = _rate_limit_store[key]
        while hits and hits[0] < window_start:
            hits.popleft()

        if len(hits) >= limit_count:
            retry_after = int(hits[0] + window_seconds - now)
            return retry_after if retry_after > 0 else 1

        hits.append(now)
        return 0


def _reset_rate_limit_store_for_tests() -> None:
    with _rate_limit_lock:
        _rate_limit_store.clear()


def _build_reset_link(token: str) -> str:
    base = app.config["FRONTEND_RESET_URL"]
    join_char = "&" if "?" in base else "?"
    return f"{base}{join_char}{urlencode({'token': token})}"


def _send_reset_password_email(to_email: str, token: str) -> bool:
    smtp_host = app.config["SMTP_HOST"]
    if not smtp_host:
        # SMTP is optional in development.
        return False

    subject = "Climate Monitor password reset"
    reset_link = _build_reset_link(token)

    msg = EmailMessage()
    msg["Subject"] = subject
    msg["From"] = app.config["MAIL_FROM"]
    msg["To"] = to_email
    msg.set_content(
        "You requested a password reset.\n\n"
        f"Open this link to set a new password:\n{reset_link}\n\n"
        f"This link expires in {app.config['RESET_TOKEN_TTL_MINUTES']} minutes.\n"
        "If you did not request this, you can ignore this email."
    )

    try:
        with smtplib.SMTP(smtp_host, app.config["SMTP_PORT"], timeout=10) as smtp:
            if app.config["SMTP_USE_TLS"]:
                smtp.starttls()

            smtp_username = app.config["SMTP_USERNAME"]
            smtp_password = app.config["SMTP_PASSWORD"]
            if smtp_username and smtp_password:
                smtp.login(smtp_username, smtp_password)

            smtp.send_message(msg)
            return True
    except Exception as exc:  # noqa: BLE001 - we only log and keep generic response
        app.logger.exception("Failed to send password reset email: %s", exc)
        return False


def _create_reset_token(user: User) -> str:
    now = _utcnow_naive()

    # Invalidate any active token for this user to keep only one live token.
    PasswordResetToken.query.filter_by(user_id=user.id, used_at=None).update(
        {PasswordResetToken.used_at: now}
    )

    raw_token = secrets.token_urlsafe(32)
    reset_token = PasswordResetToken(
        user_id=user.id,
        token_hash=_hash_reset_token(raw_token),
        expires_at=now + timedelta(minutes=app.config["RESET_TOKEN_TTL_MINUTES"]),
    )
    db.session.add(reset_token)
    db.session.commit()

    return raw_token


def login_required(view_func):
    @wraps(view_func)
    def _wrapped(*args, **kwargs):
        user_id = session.get("user_id")
        if not user_id:
            return jsonify({"error": "Authentication required"}), 401

        user = db.session.get(User, user_id)
        if not user:
            session.clear()
            return jsonify({"error": "Authentication required"}), 401

        return view_func(user, *args, **kwargs)

    return _wrapped


_configure_app()

with app.app_context():
    db.create_all()


def _openapi_spec_path() -> Path:
    configured = os.getenv("OPENAPI_SPEC_PATH")
    if configured:
        return Path(configured)
    candidate = Path(app.root_path) / "swagger.yaml"
    if candidate.exists() and candidate.stat().st_size > 0:
        return candidate

    repo_root_candidate = Path(app.root_path).parent / "swagger.yaml"
    if repo_root_candidate.exists():
        return repo_root_candidate

    return candidate

@app.route("/")
def hello():
    return "This is climate monitor"


@app.post("/api/auth/register")
def auth_register():
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


@app.post("/api/auth/login")
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


@app.post("/api/auth/forgot-password")
def auth_forgot_password():
    retry_after = _check_rate_limit(
        action="forgot-password",
        identity=_remote_identity(),
        limit_count=app.config["FORGOT_PASSWORD_RATE_LIMIT_COUNT"],
        window_seconds=app.config["FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"],
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
        reset_token = _create_reset_token(user)
        email_sent = _send_reset_password_email(user.email, reset_token)

    response = {
        "message": "If that email exists, a reset link has been generated"
    }
    if user and app.config["EXPOSE_RESET_TOKEN_IN_RESPONSE"]:
        response["reset_token"] = reset_token
        response["email_sent"] = email_sent

    return jsonify(response), 200


@app.post("/api/auth/reset-password")
def auth_reset_password():
    retry_after = _check_rate_limit(
        action="reset-password",
        identity=_remote_identity(),
        limit_count=app.config["RESET_PASSWORD_RATE_LIMIT_COUNT"],
        window_seconds=app.config["RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS"],
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

    token_hash = _hash_reset_token(token)
    now = _utcnow_naive()
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


@app.post("/api/auth/logout")
def auth_logout():
    session.clear()
    return "", 204


@app.get("/api/auth/me")
@login_required
def auth_me(current_user: User):
    return jsonify(current_user.to_dict()), 200


@app.get("/openapi.yaml")
def openapi_yaml():
    spec_path = _openapi_spec_path()
    if not spec_path.exists():
        return {"error": f"OpenAPI spec not found at {spec_path}"}, 404
    return send_file(spec_path, mimetype="application/yaml")


@app.get("/swagger")
@app.get("/swagger/")
def swagger_ui():
    html = """<!doctype html>
<html lang=\"en\">
    <head>
        <meta charset=\"utf-8\" />
        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
        <title>Climate Monitor API Docs</title>
        <link rel=\"stylesheet\" href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\" />
    </head>
    <body>
        <div id=\"swagger-ui\"></div>
        <script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\"></script>
        <script>
            window.onload = () => {
                SwaggerUIBundle({
                    url: '/openapi.yaml',
                    dom_id: '#swagger-ui',
                });
            };
        </script>
    </body>
</html>
"""
    return Response(html, mimetype="text/html")