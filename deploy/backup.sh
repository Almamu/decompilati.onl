#!/bin/sh
# Daily PostgreSQL backup (run by decompsite-backup.timer as the decompsite user).
# Writes a compressed custom-format dump and keeps the last $KEEP_DAYS days.
# Restore with: pg_restore --clean --if-exists -d decompsite <file>
set -eu
DIR="${BACKUP_DIR:-/var/backups/decompsite}"
KEEP_DAYS="${KEEP_DAYS:-14}"
DB="${PGDATABASE:-decompsite}"

mkdir -p "$DIR"
file="$DIR/$DB-$(date -u +%Y%m%d-%H%M%S).dump"
# Dump to a temporary name first so a failed run never leaves a truncated backup behind.
pg_dump -Fc -d "$DB" -f "$file.partial"
mv "$file.partial" "$file"
find "$DIR" -name "$DB-*.dump" -mtime +"$KEEP_DAYS" -delete
echo "backup written: $file ($(du -h "$file" | cut -f1))"
