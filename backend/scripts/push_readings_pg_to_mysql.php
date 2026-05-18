<?php
declare(strict_types=1);

function loadEnvFile(string $path): void
{
    if (!is_file($path)) {
        return;
    }
    $lines = file($path, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES);
    if ($lines === false) {
        return;
    }
    foreach ($lines as $line) {
        $line = trim($line);
        if ($line === '' || str_starts_with($line, '#') || !str_contains($line, '=')) {
            continue;
        }
        [$key, $value] = explode('=', $line, 2);
        $key = trim($key);
        $value = trim($value);
        if ($value !== '' && (($value[0] === '"' && str_ends_with($value, '"')) || ($value[0] === "'" && str_ends_with($value, "'")))) {
            $value = substr($value, 1, -1);
        }
        $_ENV[$key] = $value;
        $_SERVER[$key] = $value;
        putenv($key . '=' . $value);
    }
}

function envValue(string $key, ?string $default = null): ?string
{
    $value = $_ENV[$key] ?? $_SERVER[$key] ?? getenv($key);
    if ($value === false || $value === null || $value === '') {
        return $default;
    }
    return (string) $value;
}

function logLine(string $message): void
{
    $now = (new DateTimeImmutable('now', new DateTimeZone('UTC')))->format('Y-m-d H:i:s');
    fwrite(STDOUT, '[' . $now . ' UTC] ' . $message . PHP_EOL);
}

function getRequired(string $key): string
{
    $value = envValue($key);
    if ($value === null || $value === '') {
        throw new RuntimeException('Missing required env variable: ' . $key);
    }
    return $value;
}

function readStateFile(string $path): int
{
    if (!is_file($path)) {
        return -1;
    }
    $raw = file_get_contents($path);
    if ($raw === false || trim($raw) === '') {
        return -1;
    }
    $decoded = json_decode($raw, true);
    if (!is_array($decoded) || !isset($decoded['last_id'])) {
        return -1;
    }
    return (int) $decoded['last_id'];
}

function writeStateFile(string $path, int $lastId): void
{
    $payload = json_encode(['last_id' => $lastId], JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES);
    if ($payload === false) {
        throw new RuntimeException('Failed to encode sync state JSON');
    }
    if (file_put_contents($path, $payload . PHP_EOL, LOCK_EX) === false) {
        throw new RuntimeException('Failed to write sync state file: ' . $path);
    }
}

if (!extension_loaded('pdo_pgsql')) { fwrite(STDERR, "Missing PHP extension: pdo_pgsql\n"); exit(1); }
if (!extension_loaded('pdo_mysql')) { fwrite(STDERR, "Missing PHP extension: pdo_mysql\n"); exit(1); }

try {
    $envArg = $argv[1] ?? (__DIR__ . '/push_readings_pg_to_mysql.env');
    loadEnvFile($envArg);

    $stateFile = envValue('SYNC_STATE_FILE', __DIR__ . '/push_readings_state.json') ?? (__DIR__ . '/push_readings_state.json');
    $batchSize = max(1, (int) (envValue('SYNC_BATCH_SIZE', '2000') ?? '2000'));

    $pgDsn = sprintf(
        'pgsql:host=%s;port=%d;dbname=%s;sslmode=%s',
        getRequired('SRC_PG_HOST'),
        (int)(envValue('SRC_PG_PORT', '5432') ?? '5432'),
        getRequired('SRC_PG_DB'),
        envValue('SRC_PG_SSLMODE', 'prefer') ?? 'prefer'
    );
    $pgsql = new PDO($pgDsn, getRequired('SRC_PG_USER'), getRequired('SRC_PG_PASSWORD'), [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
        PDO::ATTR_EMULATE_PREPARES => false,
    ]);

    $myDsn = sprintf(
        'mysql:host=%s;port=%d;dbname=%s;charset=utf8mb4',
        getRequired('TARGET_MYSQL_HOST'),
        (int)(envValue('TARGET_MYSQL_PORT', '3306') ?? '3306'),
        getRequired('TARGET_MYSQL_DB')
    );
    $mysql = new PDO($myDsn, getRequired('TARGET_MYSQL_USER'), getRequired('TARGET_MYSQL_PASSWORD'), [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
        PDO::ATTR_EMULATE_PREPARES => false,
    ]);
    $mysql->exec("SET time_zone = '+00:00'");

    $tablesToSync = ['readings', 'devices', 'locations', 'device_shares', 'password_reset_tokens'];

    foreach ($tablesToSync as $table) {
        $stateLastId = readStateFile($stateFile . '_' . $table);
        $lastId = $stateLastId >= 0
            ? $stateLastId
            : (int)($mysql->query("SELECT COALESCE(MAX(id), 0) AS max_id FROM $table")->fetch()['max_id'] ?? 0);

        $fetchStmt = $pgsql->prepare("SELECT * FROM $table WHERE id > :last_id ORDER BY id ASC LIMIT :limit");

        $total = 0;
        while (true) {
            $fetchStmt->bindValue(':last_id', $lastId, PDO::PARAM_INT);
            $fetchStmt->bindValue(':limit', $batchSize, PDO::PARAM_INT);
            $fetchStmt->execute();
            $rows = $fetchStmt->fetchAll();

            if ($rows === []) break;

            $mysql->beginTransaction();
            try {
                foreach ($rows as $row) {
                    $insertStmt = $mysql->prepare(
                        "INSERT INTO $table (" . implode(", ", array_keys($row)) . ")
                        VALUES (" . implode(", ", array_map(fn($col) => ":$col", array_keys($row))) . ")
                        ON DUPLICATE KEY UPDATE " . implode(", ", array_map(fn($col) => "$col = VALUES($col)", array_keys($row))) . ";"
                    );

                    $insertStmt->execute(array_combine(
                        array_map(fn($col) => ":$col", array_keys($row)),
                        array_values($row)
                    ));
                    $lastId = (int)$row['id'];
                    $total++;
                }
                $mysql->commit();
                writeStateFile($stateFile . '_' . $table, $lastId);
                logLine("Table $table synced. last_id=$lastId, total=$total");
            } catch (Throwable $e) {
                $mysql->rollBack();
                throw $e;
            }
        }
    }

    logLine('Sync completed for all tables.');
    exit(0);
} catch (Throwable $e) {
    fwrite(STDERR, 'Sync failed: ' . $e->getMessage() . PHP_EOL);
    exit(1);
}