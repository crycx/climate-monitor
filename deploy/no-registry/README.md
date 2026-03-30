# Climate Monitor no-registry Docker bundle

This folder is meant to be copied to the Ubuntu server **without** the full repository and **without** using a container registry.

## Contents

- `climate-monitor-images.tar` (generated) — Docker images export (API + Postgres)
- `docker-compose.no-registry.yaml` — Compose file that runs API + Postgres with no bind-mounts
- `.env.example` — environment template (copy to `.env` on the server)

## On your build machine (Windows)

1. Build images:

```powershell
# From repo root

docker build -t climate-monitor-api:no-registry .\backend

docker build -t climate-monitor-postgres:17 .\deploy\postgres
```

2. Export images into a single tar:

```powershell
mkdir -Force .\deploy\no-registry | Out-Null

docker save -o .\deploy\no-registry\climate-monitor-images.tar climate-monitor-api:no-registry climate-monitor-postgres:17
```

3. (Optional) Zip the folder for transfer:

```powershell
Compress-Archive -Path .\deploy\no-registry\* -DestinationPath .\deploy\climate-monitor-no-registry.zip -Force
```

## On the Ubuntu server

1. Copy the bundle (`deploy/no-registry/` or the zip) to the server, e.g. to `/opt/climate-monitor/`.

2. Load images:

```bash
docker load -i /opt/climate-monitor/climate-monitor-images.tar
```

3. Create `.env`:

```bash
cd /opt/climate-monitor
cp .env.example .env
nano .env
```

4. Start containers:

```bash
docker compose -f docker-compose.no-registry.yaml up -d
```

## Nginx reverse proxy

The API is bound to `127.0.0.1:${APP_PORT}`. Configure Nginx on the server to proxy `/api` to it.
