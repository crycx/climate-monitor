import os
from pathlib import Path

from dotenv import load_dotenv


def to_bool(value: str | None, default: bool = False) -> bool:
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def database_url() -> str:
    configured = os.getenv("DATABASE_URL")
    if configured:
        return configured

    db_name = os.getenv("POSTGRES_DB")
    db_password = os.getenv("POSTGRES_PASSWORD")
    db_user = os.getenv("POSTGRES_USER", "postgres")
    db_host = os.getenv("POSTGRES_HOST", "localhost")
    db_port = os.getenv("POSTGRES_PORT", "5432")

    if db_name and db_password:
        return f"postgresql://{db_user}:{db_password}@{db_host}:{db_port}/{db_name}"

    # Local fallback for development if env vars are not available.
    return "sqlite:///climate_monitor.db"


def load_environment(app_root_path: str) -> None:
    root = Path(app_root_path)
    package_env = root / ".env"
    backend_env = root.parent / ".env"
    repo_env = root.parent.parent / ".env"

    load_dotenv(package_env, override=False)
    load_dotenv(backend_env, override=False)
    load_dotenv(repo_env, override=False)
