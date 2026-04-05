from datetime import datetime, timedelta, timezone
import secrets

from flask import Blueprint, jsonify, request
from sqlalchemy.exc import IntegrityError

from ..decorators.api_key import api_key_required
from ..decorators.auth import login_required
from ..extensions import db
from ..models import Device, Location, Reading, User


readings_bp = Blueprint("readings", __name__, url_prefix="/api/readings")
devices_bp = Blueprint("devices", __name__, url_prefix="/api/devices")


def _parse_iso_timestamp(value: str, *, parameter_name: str):
    normalized = value.strip()
    if normalized.endswith("Z"):
        normalized = normalized[:-1] + "+00:00"

    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError:
        return None, (jsonify({"error": f"Invalid {parameter_name}; expected ISO-8601 timestamp"}), 400)

    if parsed.tzinfo is not None:
        parsed = parsed.astimezone(timezone.utc).replace(tzinfo=None)

    return parsed, None


def _period_window(period: str):
    utc_now = datetime.now(timezone.utc)
    today_start = datetime(
        utc_now.year,
        utc_now.month,
        utc_now.day,
    )

    period_value = period.strip().lower()
    if period_value == "today":
        return today_start, None
    if period_value == "yesterday":
        start = today_start - timedelta(days=1)
        end = today_start
        return start, end
    if period_value == "last_24h":
        start = (utc_now - timedelta(hours=24)).replace(tzinfo=None)
        return start, None

    return None, (jsonify({"error": "Invalid period; use today, yesterday, or last_24h"}), 400)


def _apply_readings_filters(query):
    limit = request.args.get("limit", 100, type=int)
    if limit is None or limit < 1:
        return None, None, (jsonify({"error": "limit must be a positive integer"}), 400)
    clamped_limit = min(limit, 1000)

    order = (request.args.get("order") or "desc").strip().lower()
    if order not in {"asc", "desc"}:
        return None, None, (jsonify({"error": "order must be asc or desc"}), 400)

    location_id = request.args.get("location_id", type=int)
    sensor_type = request.args.get("sensor_type")
    after_id = request.args.get("after_id", type=int)
    after_ts_raw = request.args.get("after_ts")
    from_ts_raw = request.args.get("from_ts")
    to_ts_raw = request.args.get("to_ts")
    period = request.args.get("period")

    if period and (from_ts_raw or to_ts_raw):
        return None, None, (jsonify({"error": "period cannot be combined with from_ts/to_ts"}), 400)

    if location_id is not None:
        query = query.filter(Reading.location_id == location_id)
    if sensor_type:
        query = query.filter(Reading.sensor_type == sensor_type)
    if after_id is not None:
        query = query.filter(Reading.id > after_id)

    if after_ts_raw:
        after_ts, error = _parse_iso_timestamp(after_ts_raw, parameter_name="after_ts")
        if error:
            return None, None, error
        query = query.filter(Reading.timestamp > after_ts)

    if period:
        start, end_or_error = _period_window(period)
        if isinstance(end_or_error, tuple):
            return None, None, end_or_error
        end = end_or_error
        query = query.filter(Reading.timestamp >= start)
        if end is not None:
            query = query.filter(Reading.timestamp < end)
    else:
        if from_ts_raw:
            from_ts, error = _parse_iso_timestamp(from_ts_raw, parameter_name="from_ts")
            if error:
                return None, None, error
            query = query.filter(Reading.timestamp >= from_ts)

        if to_ts_raw:
            to_ts, error = _parse_iso_timestamp(to_ts_raw, parameter_name="to_ts")
            if error:
                return None, None, error
            query = query.filter(Reading.timestamp <= to_ts)

    ordering = Reading.timestamp.asc() if order == "asc" else Reading.timestamp.desc()
    query = query.order_by(ordering)

    return query, clamped_limit, None


def _parse_readings_payload(payload, *, expect_array: bool):
    if expect_array:
        if not payload or not isinstance(payload, list):
            return None, (jsonify({"error": "Expected a JSON array of readings"}), 400)
        items = payload
    else:
        if not payload or not isinstance(payload, dict):
            return None, (jsonify({"error": "Expected a JSON object reading"}), 400)
        items = [payload]

    normalized_items = []
    for item in items:
        sensor_type = item.get("sensor_type")
        value = item.get("value")
        unit = item.get("unit")

        if not sensor_type or value is None or not unit:
            return None, (jsonify({"error": "Each reading must have sensor_type, value and unit"}), 400)

        normalized_items.append({
            "sensor_type": sensor_type,
            "value": value,
            "unit": unit,
        })

    return normalized_items, None


def _save_readings(device: Device, readings_payload: list[dict]) -> int:
    readings = []
    for item in readings_payload:
        readings.append(Reading(
            device_id=device.id,
            location_id=device.location_id,
            sensor_type=item["sensor_type"],
            value=item["value"],
            unit=item["unit"],
            timestamp=datetime.now(timezone.utc),
        ))

    db.session.add_all(readings)
    db.session.commit()
    return len(readings)


def _can_access_device(current_user: User, device: Device) -> bool:
    return current_user.is_admin or device.user_id == current_user.id


def _resolve_location_for_user(current_user: User, location_id: int | None):
    if location_id is None:
        return None, None

    location = db.session.get(Location, location_id)
    if not location:
        return None, (jsonify({"error": "Location not found"}), 404)

    if not current_user.is_admin and location.user_id != current_user.id:
        return None, (jsonify({"error": "Access denied"}), 403)

    return location, None


@readings_bp.post("")
@api_key_required
def create_reading(device: Device):
    """
    POST /api/readings
    Header: X-API-Key: <device api key>
    Body: [{"sensor_type": "temperature", "value": 23.5, "unit": "C"}, ...]
    """
    payload = request.get_json(silent=True)
    readings_payload, error = _parse_readings_payload(payload, expect_array=True)
    if error:
        return error

    saved_count = _save_readings(device, readings_payload)
    return jsonify({"saved": saved_count}), 201


@devices_bp.post("/<int:device_id>/readings")
@api_key_required
def create_device_reading(device: Device, device_id: int):
    """
    Legacy compatibility endpoint for payloads that post to /api/devices/<id>/readings.
    Header: X-API-Key: <device api key>
    Body: {"sensor_type": "temperature", "value": 23.5, "unit": "C"}
    """
    if device.id != device_id:
        return jsonify({"error": "Device ID does not match API key"}), 403

    payload = request.get_json(silent=True)
    readings_payload, error = _parse_readings_payload(payload, expect_array=False)
    if error:
        return error

    saved_count = _save_readings(device, readings_payload)
    return jsonify({"saved": saved_count}), 201


@devices_bp.get("")
@login_required
def list_devices(current_user: User):
    if current_user.is_admin:
        devices = Device.query.order_by(Device.id.asc()).all()
    else:
        devices = Device.query.filter_by(user_id=current_user.id).order_by(Device.id.asc()).all()
    return jsonify([d.to_dict() for d in devices]), 200


@devices_bp.post("")
@login_required
def create_device(current_user: User):
    payload = request.get_json(silent=True) or {}

    name = (payload.get("name") or "").strip()
    if not name:
        return jsonify({"error": "name is required"}), 400

    user_id = payload.get("user_id") if current_user.is_admin else current_user.id
    if not isinstance(user_id, int) or not db.session.get(User, user_id):
        return jsonify({"error": "Invalid user_id"}), 400

    location_id = payload.get("location_id")
    if location_id is not None and not isinstance(location_id, int):
        return jsonify({"error": "location_id must be an integer"}), 400

    location, location_error = _resolve_location_for_user(current_user, location_id)
    if location_error:
        return location_error
    if location and location.user_id not in {None, user_id}:
        return jsonify({"error": "Location does not belong to selected user"}), 400

    api_key = (payload.get("api_key") or "").strip() or secrets.token_hex(32)
    if len(api_key) != 64:
        return jsonify({"error": "api_key must be 64 characters"}), 400

    now = datetime.now(timezone.utc)
    device = Device(
        user_id=user_id,
        location_id=location_id,
        name=name,
        device_type=(payload.get("device_type") or None),
        api_key=api_key,
        is_active=bool(payload.get("is_active", True)),
        created_at=now,
        updated_at=now,
    )

    db.session.add(device)
    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "api_key already exists"}), 409

    result = device.to_dict()
    result["api_key"] = device.api_key
    return jsonify(result), 201


@devices_bp.get("/<int:device_id>")
@login_required
def get_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    result = device.to_dict()
    result["api_key"] = device.api_key
    return jsonify(result), 200


@devices_bp.patch("/<int:device_id>")
@login_required
def update_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    payload = request.get_json(silent=True) or {}

    if "name" in payload:
        name = (payload.get("name") or "").strip()
        if not name:
            return jsonify({"error": "name cannot be empty"}), 400
        device.name = name

    if "device_type" in payload:
        device.device_type = payload.get("device_type") or None

    if "location_id" in payload:
        location_id = payload.get("location_id")
        if location_id is not None and not isinstance(location_id, int):
            return jsonify({"error": "location_id must be an integer"}), 400
        location, location_error = _resolve_location_for_user(current_user, location_id)
        if location_error:
            return location_error
        if location and location.user_id not in {None, device.user_id}:
            return jsonify({"error": "Location does not belong to device owner"}), 400
        device.location_id = location_id

    if "is_active" in payload:
        device.is_active = bool(payload.get("is_active"))

    if "api_key" in payload:
        api_key = (payload.get("api_key") or "").strip()
        if len(api_key) != 64:
            return jsonify({"error": "api_key must be 64 characters"}), 400
        device.api_key = api_key

    device.updated_at = datetime.now(timezone.utc)

    try:
        db.session.commit()
    except IntegrityError:
        db.session.rollback()
        return jsonify({"error": "api_key already exists"}), 409

    result = device.to_dict()
    result["api_key"] = device.api_key
    return jsonify(result), 200


@devices_bp.delete("/<int:device_id>")
@login_required
def delete_device(current_user: User, device_id: int):
    device = db.session.get(Device, device_id)
    if not device:
        return jsonify({"error": "Device not found"}), 404
    if not _can_access_device(current_user, device):
        return jsonify({"error": "Access denied"}), 403

    db.session.delete(device)
    db.session.commit()
    return "", 204


@devices_bp.get("/<int:device_id>/readings")
@login_required
def device_readings(current_user: User, device_id: int):
    device = Device.query.get_or_404(device_id)
    if not current_user.is_admin and device.user_id != current_user.id:
        return jsonify({"error": "Access denied"}), 403

    query = Reading.query.filter_by(device_id=device_id)
    query, clamped_limit, error = _apply_readings_filters(query)
    if error:
        return error

    readings = query.limit(clamped_limit).all()
    return jsonify([r.to_dict() for r in readings]), 200


@readings_bp.get("/latest")
@login_required
def latest_readings(current_user: User):
    query = Reading.query.join(Device, Reading.device_id == Device.id)
    if not current_user.is_admin:
        query = query.filter(Device.user_id == current_user.id)

    query, clamped_limit, error = _apply_readings_filters(query)
    if error:
        return error

    readings = query.limit(clamped_limit).all()
    return jsonify([r.to_dict() for r in readings]), 200
