from flask import Flask

from .config import configure_app
from .extensions import db
from .routes import register_blueprints
from .services.user_service import ensure_users_is_admin_column
from .utils.env_utils import load_environment


def create_app() -> Flask:
    app = Flask(__name__)

    load_environment(app.root_path)
    configure_app(app)
    db.init_app(app)
    register_blueprints(app)

    with app.app_context():
        db.create_all()
        ensure_users_is_admin_column()

    return app
