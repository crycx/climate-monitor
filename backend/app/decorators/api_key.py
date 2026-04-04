from functools import wraps
from datetime import datetime, timezone

from flask import jsonify, request

from ..extensions import db
from ..models import Device


def api_key_required(view_func):
    @wraps(view_func)
    def _wrapped(*args, **kwargs):
        api_key = request.headers.get("X-API-Key")
        if not api_key:
            return jsonify({"error": "API key required"}), 401

        device = Device.query.filter_by(api_key=api_key, is_active=True).first()
        if not device:
            return jsonify({"error": "Invalid or inactive API key"}), 401

        device.last_seen = datetime.now(timezone.utc)
        db.session.commit()

        return view_func(device, *args, **kwargs)

    return _wrapped
