from flask import Flask

from .auth import auth_bp
from .docs import docs_bp
from .readings import devices_bp, readings_bp
from .users import users_bp


def register_blueprints(app: Flask) -> None:
    app.register_blueprint(auth_bp)
    app.register_blueprint(users_bp)
    app.register_blueprint(docs_bp)
    app.register_blueprint(readings_bp)
    app.register_blueprint(devices_bp)
