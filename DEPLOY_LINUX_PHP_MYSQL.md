# Climate Monitor Linux Deployment Without Docker

This guide shows how to run the PHP backend and MySQL directly on a Linux server without Docker.

Tested approach:
- MySQL installed from the OS package manager
- PHP 8.x running with the built-in server for local checks or PHP-FPM for production
- Nginx as reverse proxy
- Ubuntu or Debian style commands

## 1. Install system packages

```bash
sudo apt update
sudo apt install -y mysql-server php php-cli php-mysql nginx
```

If you want a quicker local smoke test only, you can skip Nginx and use the built-in PHP server.

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

## 3. Create the MySQL database

Open the MySQL shell:

```bash
sudo mysql
```

Run:

```sql
CREATE DATABASE climate_monitor CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
CREATE USER 'climate_monitor_user'@'localhost' IDENTIFIED BY 'vali-siia-tugev-salas6na';
GRANT ALL PRIVILEGES ON climate_monitor.* TO 'climate_monitor_user'@'localhost';
FLUSH PRIVILEGES;
EXIT;
```

## 4. Import the schema

```bash
mysql -u climate_monitor_user -p climate_monitor < /opt/climate_monitor/init.sql
```

## 5. Create the environment file

Create `/opt/climate_monitor/.env`.

Example:

```env
APP_PORT=8000
DB_HOST=127.0.0.1
DB_PORT=3306
DB_NAME=climate_monitor
DB_USER=climate_monitor_user
DB_PASSWORD=vali-siia-tugev-salas6na

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
SELF_REGISTRATION_ENABLED=false
```

## 6. Test the backend manually

Run this once before creating a service:

```bash
cd /opt/climate_monitor
php -S 127.0.0.1:8000 -t backend/public
```

Then test from the server:

```bash
curl http://127.0.0.1:8000/
curl http://127.0.0.1:8000/openapi.yaml
```

## 7. Create a systemd service

Create the service file:

```bash
sudo nano /etc/systemd/system/climate-monitor.service
```

Example using the PHP built-in server for a simple deployment:

```ini
[Unit]
Description=Climate Monitor PHP backend
After=network.target mysql.service
Wants=mysql.service

[Service]
WorkingDirectory=/opt/climate_monitor
ExecStart=/usr/bin/php -S 127.0.0.1:8000 -t /opt/climate_monitor/backend/public
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

## 8. Put Nginx in front of the app

Create a site config:

```bash
sudo nano /etc/nginx/sites-available/climate-monitor
```

Example config:

```nginx
server {
    listen 80;
    server_name sinu-domeen.ee;

    location /api/ {
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    location /swagger {
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;
    }

    location /openapi.yaml {
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;
    }
}
```

Enable the site:

```bash
sudo ln -s /etc/nginx/sites-available/climate-monitor /etc/nginx/sites-enabled/climate-monitor
sudo nginx -t
sudo systemctl reload nginx
```

## 9. Manual checks

- `GET /` returns a JSON status message
- `GET /openapi.yaml` returns the OpenAPI file
- `POST /api/auth/login` sets the session cookie
- `POST /api/readings` accepts `X-API-Key`

If the database schema changes, update `init.sql` and re-import it on the server.
