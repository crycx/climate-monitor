import os
from datetime import timedelta

from flask import Flask
from flask_cors import CORS

from .utils.env_utils import database_url, to_bool


def configure_app(app: Flask) -> None:
    frontend_origin = os.getenv("FRONTEND_ORIGIN", "http://localhost:4200")

    app.config["SQLALCHEMY_DATABASE_URI"] = database_url()
    app.config["SQLALCHEMY_TRACK_MODIFICATIONS"] = False
    app.config["SECRET_KEY"] = os.getenv("SECRET_KEY", "dev-only-secret-change-me")
    app.config["SESSION_COOKIE_HTTPONLY"] = True
    app.config["SESSION_COOKIE_SAMESITE"] = os.getenv("SESSION_COOKIE_SAMESITE", "Lax")
    app.config["SESSION_COOKIE_SECURE"] = to_bool(os.getenv("SESSION_COOKIE_SECURE"), False)
    app.config["PERMANENT_SESSION_LIFETIME"] = timedelta(
        hours=int(os.getenv("SESSION_LIFETIME_HOURS", "24"))
    )
    app.config["RESET_TOKEN_TTL_MINUTES"] = int(os.getenv("RESET_TOKEN_TTL_MINUTES", "30"))
    app.config["EXPOSE_RESET_TOKEN_IN_RESPONSE"] = to_bool(
        os.getenv("EXPOSE_RESET_TOKEN_IN_RESPONSE"),
        to_bool(os.getenv("FLASK_DEBUG"), False),
    )
    app.config["FRONTEND_RESET_URL"] = os.getenv(
        "FRONTEND_RESET_URL", "http://localhost:4200/reset-password"
    )
    app.config["SMTP_HOST"] = os.getenv("SMTP_HOST", "")
    app.config["SMTP_PORT"] = int(os.getenv("SMTP_PORT", "587"))
    app.config["SMTP_USERNAME"] = os.getenv("SMTP_USERNAME", "")
    app.config["SMTP_PASSWORD"] = os.getenv("SMTP_PASSWORD", "")
    app.config["SMTP_USE_TLS"] = to_bool(os.getenv("SMTP_USE_TLS"), True)
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
    app.config["SELF_REGISTRATION_ENABLED"] = to_bool(
        os.getenv("SELF_REGISTRATION_ENABLED"),
        True,
    )

    CORS(
        app,
        supports_credentials=True,
        resources={r"/api/*": {"origins": [frontend_origin]}},
    )
