#!/bin/bash

# MySQL andmete import SQL dump formaadist
MYSQL_USER="mysql_user"
MYSQL_DB="database_name"
IMPORT_PATH="backup.sql"

mysql -u $MYSQL_USER -p $MYSQL_DB < $IMPORT_PATH

echo "Andmed on imporditud MySQL andmebaasi: $MYSQL_DB"