# Climate Monitor
Smart home system for monitoring temperature and humidity using ESP32 + DHT22 sensors.

## Local development
1. Create .env folder with 
    - APP_PORT
    - POSTGRES_PASSWORD
    - POSTGRES_DB
2. Run `docker-compose up` or with -d flag for detached
3. Create database tables from `init.sql` in the repo root:
     - PowerShell:
         - `Get-Content .\init.sql -Raw | docker exec -i db psql -U postgres -d climate_monitor`
     - cmd.exe:
         - `type init.sql | docker exec -i db psql -U postgres -d climate_monitor`
         - or `cmd /c "docker exec -i db psql -U postgres -d climate_monitor < init.sql"`
     - Bash/zsh:
         - `docker exec -i db psql -U postgres -d climate_monitor < init.sql`

## Tech Stack

Hardware: ESP32 with a DHT22 sensor (Rust). The
Backend: Python Flask API
Frontend: Angular.
Database: PostgreSQL

## Mermaid ERP diagram

erDiagram
    Users ||--o{ Locations : has
    Users ||--o{ Devices : owns
    Locations ||--o{ Devices : contains
    Devices ||--o{ Readings : records

    Users {
        int id PK
        varchar username UK
        varchar email UK
        varchar password_hash
        timestamp created_at
        timestamp updated_at
    }

    Locations {
        int id PK
        int user_id FK
        varchar name
        text description
        timestamp created_at
        timestamp updated_at
    }

    Devices {
        int id PK
        int user_id FK
        int location_id FK
        varchar name
        varchar device_type
        varchar api_key UK
        boolean is_active
        timestamp last_seen
        timestamp created_at
        timestamp updated_at
    }

    Readings {
        bigint id PK
        int device_id FK
        varchar sensor_type
        float value
        varchar unit
        timestamp timestamp
    }

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

