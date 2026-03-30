FROM postgres:17

# Initialize schema on first DB startup.
# This runs only when the data directory is empty.
COPY init.sql /docker-entrypoint-initdb.d/init.sql
