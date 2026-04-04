from ..extensions import db


class Reading(db.Model):
    __tablename__ = "readings"

    id = db.Column(db.BigInteger, primary_key=True)
    device_id = db.Column(db.Integer, db.ForeignKey("devices.id", ondelete="CASCADE"), nullable=False)
    sensor_type = db.Column(db.String(50), nullable=False)
    value = db.Column(db.Numeric(10, 4), nullable=False)
    unit = db.Column(db.String(20), nullable=False)
    timestamp = db.Column(db.DateTime)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "device_id": self.device_id,
            "sensor_type": self.sensor_type,
            "value": float(self.value),
            "unit": self.unit,
            "timestamp": self.timestamp.isoformat() if self.timestamp else None,
        }
