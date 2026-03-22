import smtplib
from email.message import EmailMessage
from urllib.parse import urlencode

from flask import current_app


def build_reset_link(token: str) -> str:
    base = current_app.config["FRONTEND_RESET_URL"]
    join_char = "&" if "?" in base else "?"
    return f"{base}{join_char}{urlencode({'token': token})}"


def send_reset_password_email(to_email: str, token: str) -> bool:
    smtp_host = current_app.config["SMTP_HOST"]
    if not smtp_host:
        # SMTP is optional in development.
        return False

    subject = "Climate Monitor password reset"
    reset_link = build_reset_link(token)

    msg = EmailMessage()
    msg["Subject"] = subject
    msg["From"] = current_app.config["MAIL_FROM"]
    msg["To"] = to_email
    msg.set_content(
        "You requested a password reset.\n\n"
        f"Open this link to set a new password:\n{reset_link}\n\n"
        f"This link expires in {current_app.config['RESET_TOKEN_TTL_MINUTES']} minutes.\n"
        "If you did not request this, you can ignore this email."
    )

    try:
        with smtplib.SMTP(
            smtp_host,
            current_app.config["SMTP_PORT"],
            timeout=10,
        ) as smtp:
            if current_app.config["SMTP_USE_TLS"]:
                smtp.starttls()

            smtp_username = current_app.config["SMTP_USERNAME"]
            smtp_password = current_app.config["SMTP_PASSWORD"]
            if smtp_username and smtp_password:
                smtp.login(smtp_username, smtp_password)

            smtp.send_message(msg)
            return True
    except Exception as exc:  # noqa: BLE001 - we only log and keep generic response
        current_app.logger.exception("Failed to send password reset email: %s", exc)
        return False
