import os

from app import create_app
from app.extensions import db
from app.models import PasswordResetToken, User
from app.services.rate_limit_service import (
    reset_rate_limit_store_for_tests as _reset_rate_limit_store_for_tests,
)


app = create_app()


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=int(os.getenv("APP_PORT", "5000")))
