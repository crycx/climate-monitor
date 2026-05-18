#!/bin/bash

# PostgreSQL andmete eksport SQL dump formaadis
PG_USER="postgres_user"
PG_DB="database_name"
EXPORT_PATH="backup.sql"

pg_dump -U $PG_USER -d $PG_DB -F c -f $EXPORT_PATH

echo "PostgreSQL andmed on eksporditud faili: $EXPORT_PATH"