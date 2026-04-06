from ..extensions import db


class Device(db.Model):
    __tablename__ = "devices"

    id = db.Column(db.Integer, primary_key=True)
    user_id = db.Column(db.Integer, db.ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    location_id = db.Column(db.Integer, db.ForeignKey("locations.id", ondelete="SET NULL"), nullable=True)
    name = db.Column(db.String(255), nullable=False)
    device_type = db.Column(db.String(50))
    api_key = db.Column(db.String(64), unique=True, nullable=False)
    is_active = db.Column(db.Boolean, default=True)
    last_seen = db.Column(db.DateTime)
    created_at = db.Column(db.DateTime)
    updated_at = db.Column(db.DateTime)

    readings = db.relationship("Reading", backref="device", lazy=True, cascade="all, delete-orphan")

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "user_id": self.user_id,
            "location_id": self.location_id,
            "name": self.name,
            "device_type": self.device_type,
            "is_active": self.is_active,
            "last_seen": self.last_seen.isoformat() + 'Z' if self.last_seen else None,
            "created_at": self.created_at.isoformat() + 'Z' if self.created_at else None,
        }
