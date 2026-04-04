import secrets
from datetime import timedelta

from flask import current_app

from ..extensions import db
from ..models import PasswordResetToken, User
from ..utils.datetime_utils import utcnow_naive
from ..utils.token_utils import hash_reset_token


def create_reset_token(user: User) -> str:
    now = utcnow_naive()

    # Invalidate any active token for this user to keep only one live token.
    PasswordResetToken.query.filter_by(user_id=user.id, used_at=None).update(
        {PasswordResetToken.used_at: now}
    )

    raw_token = secrets.token_urlsafe(32)
    reset_token = PasswordResetToken(
        user_id=user.id,
        token_hash=hash_reset_token(raw_token),
        expires_at=now + timedelta(minutes=current_app.config["RESET_TOKEN_TTL_MINUTES"]),
    )
    db.session.add(reset_token)
    db.session.commit()

    return raw_token
