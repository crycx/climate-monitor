# Climate Monitor Linux Deployment Without Docker

This guide shows how to run PostgreSQL and `backend/app.py` directly on a Linux server without Docker.

Tested approach:
- PostgreSQL installed from the OS package manager
- Flask app running with Gunicorn
- Nginx as reverse proxy
- Ubuntu or Debian style commands

## 1. Install system packages

```bash
sudo apt update
sudo apt install -y postgresql postgresql-contrib python3 python3-venv python3-pip nginx
```

If you do not want Nginx yet, you can leave it out and run Gunicorn directly on a port.

## 2. Copy the project to the server

Example target path:

```bash
sudo mkdir -p /opt/climate_monitor
sudo chown "$USER":"$USER" /opt/climate_monitor
```

Then copy or clone the project there.

Example:

```bash
cd /opt
git clone <YOUR_REPOSITORY_URL> climate_monitor
cd /opt/climate_monitor
```

## 3. Create PostgreSQL user and database

Open PostgreSQL shell:

```bash
sudo -u postgres psql
```

Run:

```sql
CREATE USER climate_monitor_user WITH PASSWORD 'vali-siia-tugev-salas6na';
CREATE DATABASE climate_monitor OWNER climate_monitor_user;
GRANT ALL PRIVILEGES ON DATABASE climate_monitor TO climate_monitor_user;
\q
```

## 4. Create the database schema

You have two options.

### Option A: let `app.py` create the tables automatically

When the backend starts, it runs `db.create_all()`.

Important limitation:
- It only creates tables that are defined as SQLAlchemy models in `backend/app.py`.
- At the moment those are `users` and `password_reset_tokens`.

This is enough if you only need the currently implemented auth-related tables.

### Option B: import the full `init.sql` schema

If you also want `locations`, `devices`, and `readings`, import the SQL file manually:

```bash
psql postgresql://climate_monitor_user:vali-siia-tugev-salas6na@localhost:5432/climate_monitor -f /opt/climate_monitor/init.sql
```

If your app later uses both the SQLAlchemy-created tables and the extra schema from `init.sql`, keep the schema consistent. Right now this project does not use migrations, so schema changes are manual.

## 5. Create a Python virtual environment

```bash
cd /opt/climate_monitor/backend
python3 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install -r requirements.txt
pip install gunicorn
```

Why Gunicorn:
- `flask run` is acceptable for local development
- Gunicorn is the safer production choice on Linux

## 6. Create the environment file

`backend/app.py` loads `.env` from two places:
- `backend/.env`
- repository root `.env`

Simplest choice: create `/opt/climate_monitor/.env`

Example:

```env
APP_PORT=8000

DATABASE_URL=postgresql://climate_monitor_user:vali-siia-tugev-salas6na@localhost:5432/climate_monitor

SECRET_KEY=asenda-see-pika-juhusliku-v22rtusega
FRONTEND_ORIGIN=https://sinu-domeen.ee
SESSION_COOKIE_SECURE=true
SESSION_COOKIE_SAMESITE=Lax
SESSION_LIFETIME_HOURS=24

FRONTEND_RESET_URL=https://sinu-domeen.ee/reset-password
RESET_TOKEN_TTL_MINUTES=30
EXPOSE_RESET_TOKEN_IN_RESPONSE=false

SMTP_HOST=smtp.sinu-teenus.ee
SMTP_PORT=587
SMTP_USERNAME=smtp-kasutaja
SMTP_PASSWORD=smtp-salas6na
SMTP_USE_TLS=true
MAIL_FROM=no-reply@sinu-domeen.ee

FORGOT_PASSWORD_RATE_LIMIT_COUNT=5
FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
RESET_PASSWORD_RATE_LIMIT_COUNT=10
RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
```

Example with Zone.ee email hosting:

```env
APP_PORT=8000

DATABASE_URL=postgresql://climate_monitor_user:vali-siia-tugev-salas6na@localhost:5432/climate_monitor

SECRET_KEY=asenda-see-pika-juhusliku-v22rtusega
FRONTEND_ORIGIN=https://sinu-domeen.ee
SESSION_COOKIE_SECURE=true
SESSION_COOKIE_SAMESITE=Lax
SESSION_LIFETIME_HOURS=24

FRONTEND_RESET_URL=https://sinu-domeen.ee/reset-password
RESET_TOKEN_TTL_MINUTES=30
EXPOSE_RESET_TOKEN_IN_RESPONSE=false

# Replace the host with the SMTP server shown in your Zone.ee mailbox settings.
SMTP_HOST=<ZONE_SMTP_HOST>
SMTP_PORT=587
SMTP_USERNAME=no-reply@sinu-domeen.ee
SMTP_PASSWORD=<ZONE_MAILBOX_PASSWORD>
SMTP_USE_TLS=true
MAIL_FROM=no-reply@sinu-domeen.ee

FORGOT_PASSWORD_RATE_LIMIT_COUNT=5
FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
RESET_PASSWORD_RATE_LIMIT_COUNT=10
RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS=900
```

Notes:
- Prefer `DATABASE_URL` on Linux deployment. It is the most explicit option.
- If `DATABASE_URL` is missing, the app falls back to `POSTGRES_*` variables.
- If neither is set, the app falls back to SQLite, which is usually not what you want on a server.
- For Zone.ee, use the full mailbox address as `SMTP_USERNAME`.
- This project currently uses SMTP with `STARTTLS`, so prefer port `587` with `SMTP_USE_TLS=true`.
- If Zone.ee shows only an SSL port like `465`, `backend/app.py` would need a small change to use `SMTP_SSL`.

## 7. Test the backend manually

Run this once before creating a service:

```bash
cd /opt/climate_monitor/backend
source .venv/bin/activate
gunicorn --bind 127.0.0.1:5000 app:app
```

Then test from the server:

```bash
curl http://127.0.0.1:5000/
curl http://127.0.0.1:5000/openapi.yaml
```

If the backend starts cleanly, PostgreSQL connectivity and `.env` loading are working.

## 8. Create a systemd service for the backend

Create a dedicated system user if you want stricter isolation:

```bash
sudo useradd --system --home /opt/climate_monitor --shell /usr/sbin/nologin climate_monitor
sudo chown -R climate_monitor:climate_monitor /opt/climate_monitor
```

Create the service file:

```bash
sudo nano /etc/systemd/system/climate-monitor.service
```

Add:

```ini
[Unit]
Description=Climate Monitor Flask backend
After=network.target postgresql.service
Wants=postgresql.service

[Service]
User=climate_monitor
Group=climate_monitor
WorkingDirectory=/opt/climate_monitor/backend
ExecStart=/opt/climate_monitor/backend/.venv/bin/gunicorn --workers 3 --bind 127.0.0.1:5000 app:app
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Then enable and start it:

```bash
sudo systemctl daemon-reload
sudo systemctl enable climate-monitor
sudo systemctl start climate-monitor
sudo systemctl status climate-monitor
```

Useful logs:

```bash
journalctl -u climate-monitor -f
```

## 9. Put Nginx in front of the app

Create a site config:

```bash
sudo nano /etc/nginx/sites-available/climate-monitor
```

Example config:

```nginx
server {
    listen 80;
    server_name sinu-domeen.ee;

    location / {
        proxy_pass http://127.0.0.1:5000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Enable it:

```bash
sudo ln -s /etc/nginx/sites-available/climate-monitor /etc/nginx/sites-enabled/climate-monitor
sudo nginx -t
sudo systemctl reload nginx
```

Because `app.py` reads `X-Forwarded-For`, the forgot-password and reset-password rate limiting will still use the client IP when Nginx forwards that header.

## 10. Add HTTPS

If the server is public, use HTTPS and keep this in `.env`:

```env
SESSION_COOKIE_SECURE=true
```

Typical Let's Encrypt setup:

```bash
sudo apt install -y certbot python3-certbot-nginx
sudo certbot --nginx -d sinu-domeen.ee
```

## 11. Updating the app later

After pulling new code:

```bash
cd /opt/climate_monitor/backend
source .venv/bin/activate
pip install -r requirements.txt
sudo systemctl restart climate-monitor
```

If the database schema changes, update PostgreSQL separately. This project currently has no Alembic or Flask-Migrate based migration flow.

## 12. Quick checklist

- PostgreSQL service is running
- Database and user exist
- `.env` contains a valid `DATABASE_URL`
- Python virtual environment exists
- `requirements.txt` is installed
- Gunicorn starts with `app:app`
- systemd service is enabled
- Nginx forwards requests to `127.0.0.1:5000`
- HTTPS is enabled if the service is public

## 13. Common pitfalls in this project

- `db.create_all()` does not create every table from `init.sql`
- Missing `DATABASE_URL` may cause an unexpected SQLite fallback
- `SESSION_COOKIE_SECURE=true` requires HTTPS in front of the app
- SMTP is optional, but forgot-password email sending will not work until SMTP variables are set
- With Zone.ee, do not guess the SMTP hostname; copy it from the mailbox settings in Zone control panel