from datetime import datetime, timezone

from flask import Blueprint, jsonify, request

from ..decorators.api_key import api_key_required
from ..decorators.auth import login_required
from ..extensions import db
from ..models import Device, Reading, User


readings_bp = Blueprint("readings", __name__, url_prefix="/api/readings")
devices_bp = Blueprint("devices", __name__, url_prefix="/api/devices")


@readings_bp.post("")
@api_key_required
def create_reading(device: Device):
    """
    POST /api/readings
    Header: X-API-Key: <device api key>
    Body: [{"sensor_type": "temperature", "value": 23.5, "unit": "C"}, ...]
    """
    payload = request.get_json(silent=True)
    if not payload or not isinstance(payload, list):
        return jsonify({"error": "Expected a JSON array of readings"}), 400

    readings = []
    for item in payload:
        sensor_type = item.get("sensor_type")
        value = item.get("value")
        unit = item.get("unit")

        if not sensor_type or value is None or not unit:
            return jsonify({"error": "Each reading must have sensor_type, value and unit"}), 400

        readings.append(Reading(
            device_id=device.id,
            sensor_type=sensor_type,
            value=value,
            unit=unit,
            timestamp=datetime.now(timezone.utc),
        ))

    db.session.add_all(readings)
    db.session.commit()

    return jsonify({"saved": len(readings)}), 201


@devices_bp.get("")
@login_required
def list_devices(current_user: User):
    if current_user.is_admin:
        devices = Device.query.order_by(Device.id.asc()).all()
    else:
        devices = Device.query.filter_by(user_id=current_user.id).order_by(Device.id.asc()).all()
    return jsonify([d.to_dict() for d in devices]), 200


@devices_bp.get("/<int:device_id>/readings")
@login_required
def device_readings(current_user: User, device_id: int):
    device = Device.query.get_or_404(device_id)
    if not current_user.is_admin and device.user_id != current_user.id:
        return jsonify({"error": "Access denied"}), 403

    limit = request.args.get("limit", 100, type=int)
    readings = (
        Reading.query
        .filter_by(device_id=device_id)
        .order_by(Reading.timestamp.desc())
        .limit(min(limit, 1000))
        .all()
    )
    return jsonify([r.to_dict() for r in readings]), 200
