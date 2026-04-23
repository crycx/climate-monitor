from ..extensions import db


class DeviceShare(db.Model):
    __tablename__ = "device_shares"

    id = db.Column(db.Integer, primary_key=True)
    device_id = db.Column(db.Integer, db.ForeignKey("devices.id", ondelete="CASCADE"), nullable=False)
    shared_with_user_id = db.Column(db.Integer, db.ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    starts_at = db.Column(db.DateTime, nullable=False)
    ends_at = db.Column(db.DateTime, nullable=True)
    created_at = db.Column(db.DateTime, nullable=False)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "device_id": self.device_id,
            "shared_with_user_id": self.shared_with_user_id,
            "starts_at": self.starts_at.isoformat() + 'Z' if self.starts_at else None,
            "ends_at": self.ends_at.isoformat() + 'Z' if self.ends_at else None,
            "created_at": self.created_at.isoformat() + 'Z' if self.created_at else None,
        }