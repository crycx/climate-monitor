from functools import wraps

from flask import jsonify, session

from ..extensions import db
from ..models import User


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
