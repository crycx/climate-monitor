# Climate Monitor
Smart home system for monitoring temperature and humidity using ESP32 + DHT22 sensors.

## Tech Stack

Hardware: ESP32 with a DHT22 sensor (Rust). The
Backend: Python Flask API
Frontend: Angular.
Database: PostgreSQL

## API Endpoints
https://editor.swagger.io/

### Authentication

- `POST /api/auth/register` - Register new user
- `POST /api/auth/login` - Login user
- `GET /api/auth/me` - Get current user

### Locations

- `GET /api/locations` - List user's locations
- `POST /api/locations` - Create location
- `PUT /api/locations/:id` - Update location
- `DELETE /api/locations/:id` - Delete location

### Devices

- `GET /api/devices` - List user's devices
- `POST /api/devices` - Register new device
- `GET /api/devices/:id` - Get device details
- `PUT /api/devices/:id` - Update device
- `DELETE /api/devices/:id` - Delete device

### Readings

- `GET /api/devices/:id/readings` - Get device readings
- `POST /api/devices/:id/readings` - Submit reading (from ESP32)
- `GET /api/readings/latest` - Get latest readings across all devices

