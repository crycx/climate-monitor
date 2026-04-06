from datetime import datetime, timezone

from flask import Blueprint, jsonify, request

from ..decorators.auth import login_required
from ..extensions import db
from ..models import Location, User


locations_bp = Blueprint("locations", __name__, url_prefix="/api/locations")


@locations_bp.get("")
@login_required
def list_locations(current_user: User):
    if current_user.is_admin:
        locations = Location.query.order_by(Location.id.asc()).all()
    else:
        locations = Location.query.filter_by(user_id=current_user.id).order_by(Location.id.asc()).all()

    return jsonify([location.to_dict() for location in locations]), 200


@locations_bp.post("")
@login_required
def create_location(current_user: User):
    payload = request.get_json(silent=True) or {}

    name = (payload.get("name") or "").strip()
    if not name:
        return jsonify({"error": "name is required"}), 400

    if current_user.is_admin:
        user_id = payload.get("user_id", current_user.id)
    else:
        user_id = current_user.id
    if not isinstance(user_id, int) or not db.session.get(User, user_id):
        return jsonify({"error": "Invalid user_id"}), 400

    now = datetime.now(timezone.utc)
    location = Location(
        user_id=user_id,
        name=name,
        description=(payload.get("description") or None),
        created_at=now,
        updated_at=now,
    )

    db.session.add(location)
    db.session.commit()
    return jsonify(location.to_dict()), 201


@locations_bp.get("/<int:location_id>")
@login_required
def get_location(current_user: User, location_id: int):
    location = db.session.get(Location, location_id)
    if not location:
        return jsonify({"error": "Location not found"}), 404
    if not current_user.is_admin and location.user_id != current_user.id:
        return jsonify({"error": "Access denied"}), 403

    return jsonify(location.to_dict()), 200


@locations_bp.patch("/<int:location_id>")
@login_required
def update_location(current_user: User, location_id: int):
    location = db.session.get(Location, location_id)
    if not location:
        return jsonify({"error": "Location not found"}), 404
    if not current_user.is_admin and location.user_id != current_user.id:
        return jsonify({"error": "Access denied"}), 403

    payload = request.get_json(silent=True) or {}
    if "name" in payload:
        name = (payload.get("name") or "").strip()
        if not name:
            return jsonify({"error": "name cannot be empty"}), 400
        location.name = name

    if "description" in payload:
        location.description = payload.get("description") or None

    location.updated_at = datetime.now(timezone.utc)
    db.session.commit()
    return jsonify(location.to_dict()), 200


@locations_bp.delete("/<int:location_id>")
@login_required
def delete_location(current_user: User, location_id: int):
    location = db.session.get(Location, location_id)
    if not location:
        return jsonify({"error": "Location not found"}), 404
    if not current_user.is_admin and location.user_id != current_user.id:
        return jsonify({"error": "Access denied"}), 403

    db.session.delete(location)
    db.session.commit()
    return "", 204
