# Climate Monitor
Smart home system for monitoring temperature and humidity using ESP32 + DHT22 sensors.

## ESP32 firmware docs

- See `esp32/README.md` for detailed firmware documentation, including shared components and board-specific setup for ESP32-C3 and ESP32-WROOM.
- For ESP32-C3 Super Mini boards, the built-in LED may be active-high. In that case use `LED_ACTIVE_HIGH = "1"` in `.cargo/config.toml` or run the firmware with `cargo run --release --features supermini`.

## Local development
1. Create `.env` from template:
    - PowerShell: `Copy-Item .env.example .env`
    - Bash/zsh: `cp .env.example .env`
2. Update values in `.env` (at minimum):
    - APP_PORT
    - DB_HOST
    - DB_PORT
    - DB_NAME
    - DB_USER
    - DB_PASSWORD
    - SECRET_KEY
        - FRONTEND_ORIGIN (example: http://localhost:4200)
            - Can contain multiple comma-separated origins (example: http://localhost:4200,https://climate.sinpal.ee)
            - Must include scheme (http/https) and must match browser origin exactly
    - (optional, security) SELF_REGISTRATION_ENABLED=false
    - (optional) SESSION_COOKIE_SECURE=true in production
    - (optional, reset email) FRONTEND_RESET_URL=http://localhost:4200/reset-password
    - (optional, reset email) SMTP_HOST
    - (optional, reset email) SMTP_PORT=587
    - (optional, reset email) SMTP_USERNAME
    - (optional, reset email) SMTP_PASSWORD
    - (optional, reset email) SMTP_USE_TLS=true
    - (optional, reset email) MAIL_FROM=no-reply@climate-monitor.local
    - (optional, dev only) EXPOSE_RESET_TOKEN_IN_RESPONSE=true
    - (optional, security) FORGOT_PASSWORD_RATE_LIMIT_COUNT=5
    - (optional, security) FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
    - (optional, security) RESET_PASSWORD_RATE_LIMIT_COUNT=10
    - (optional, security) RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
3. Create the MySQL schema from `init.sql` in the repo root:
    - PowerShell: `Get-Content .\init.sql -Raw | mysql -h 127.0.0.1 -u root -p climate_monitor`
    - Bash/zsh: `mysql -h 127.0.0.1 -u root -p climate_monitor < init.sql`
4. Start the backend:
    - `php -S 127.0.0.1:8000 -t backend/public`
5. Start the frontend:
    - `cd frontend/climate-monitor`
    - `npm install`
    - `npm start`

## Linux server without Docker

- See `DEPLOY_LINUX_PHP_MYSQL.md` for a step-by-step guide to run MySQL and the PHP backend directly on a Linux server without Docker.
- Recommended stack for production:
    - MySQL as a system service
    - PHP 8.x with PHP-FPM or the built-in server for the API
    - Nginx as reverse proxy

Important:
- `backend/public/index.php` serves the PHP API and expects the MySQL schema from `init.sql`.
- Import `init.sql` manually before starting the backend.

## Backend tests

- There is no PHP test suite wired up yet.
- Smoke test the API with `php -S 127.0.0.1:8000 -t backend/public` and a few `curl` requests.

## Angular quick start

1. Install dependencies:
    - `cd frontend/climate-monitor`
    - `npm install`
2. Start development server:
    - `ng serve` or `ng serve -o` to open it in browser after start
3. Open app:
    - `http://localhost:4200`

### Angular auth flow (manual test)

1. Open login page: `http://localhost:4200/login`
2. If self-registration is enabled, open register page: `http://localhost:4200/register`
3. If self-registration is enabled, create an account and confirm you are redirected to Home (`/`)
4. Logout from the header menu and confirm redirect back to `/login`
5. Forgot password flow:
    - Open `http://localhost:4200/forgot-password`
    - Submit email
    - In development you can use returned token (if `EXPOSE_RESET_TOKEN_IN_RESPONSE=true`) on `http://localhost:4200/reset-password`

### Angular environments

- Development API base URL: `frontend/climate-monitor/src/environments/environment.ts`
- Production API base URL: `frontend/climate-monitor/src/environments/environment.prod.ts`
- Build config switches environment file in `frontend/climate-monitor/angular.json`

### Production build

- `cd frontend/climate-monitor`
- `npm run build`

## Tech Stack

Hardware: ESP32 with a DHT22 sensor (Rust).
Backend: PHP API
Frontend: Angular.
Database: MySQL

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

### Swagger UI (served by the PHP backend)

- Swagger UI: `http://localhost:8000/swagger`
- OpenAPI spec: `http://localhost:8000/openapi.yaml`

### Authentication

- `POST /api/auth/register` - Register new user
- `POST /api/auth/login` - Login user
- `GET /api/auth/me` - Get current user
- `POST /api/auth/change-password` - Change current user's password

### Users

- `POST /api/users` - Create user and send password setup token by email (admin only)
- `GET /api/users` - List users (admin sees all; regular user sees self)
- `GET /api/users/:id` - Get user details (self or admin)
- `PATCH /api/users/:id` - Update user (self or admin; only admin can set `is_admin` and reset another user's password)
- `DELETE /api/users/:id` - Delete user (self or admin)

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

