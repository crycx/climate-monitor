<?php
declare(strict_types=1);

require_once dirname(__DIR__) . '/src/helpers.php';

cm_load_env(dirname(__DIR__) . '/.env');
cm_load_env(dirname(__DIR__, 2) . '/.env');

function log_line(string $message): void
{
    $now = (new DateTimeImmutable('now', new DateTimeZone('UTC')))->format('Y-m-d H:i:s');
    fwrite(STDOUT, '[' . $now . ' UTC] ' . $message . PHP_EOL);
}

function pg_pdo_from_env(): PDO
{
    $dsn = cm_env('PG_DSN');
    if ($dsn === null || $dsn === '') {
        $host = cm_env('PG_HOST', '127.0.0.1') ?? '127.0.0.1';
        $port = (int) (cm_env('PG_PORT', '5432') ?? '5432');
        $db = cm_env('PG_DB', 'climate_monitor') ?? 'climate_monitor';
        $sslMode = cm_env('PG_SSLMODE', 'prefer') ?? 'prefer';
        $dsn = sprintf('pgsql:host=%s;port=%d;dbname=%s;sslmode=%s', $host, $port, $db, $sslMode);
    }

    $user = cm_env('PG_USER', 'postgres') ?? 'postgres';
    $password = cm_env('PG_PASSWORD', '') ?? '';

    return new PDO($dsn, $user, $password, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
        PDO::ATTR_EMULATE_PREPARES => false,
    ]);
}

if (!extension_loaded('pdo_pgsql')) {
    fwrite(STDERR, "Missing PHP extension: pdo_pgsql" . PHP_EOL);
    exit(1);
}

try {
    $mysql = cm_pdo(cm_app_config());
    $pgsql = pg_pdo_from_env();

    $batchSize = (int) (cm_env('READINGS_SYNC_BATCH_SIZE', '2000') ?? '2000');
    if ($batchSize < 1) {
        $batchSize = 2000;
    }

    $tablesToSync = [
        'devices',
        'locations',
        'readings',
        'device_shares',
        'password_reset_tokens'
    ];

    foreach ($tablesToSync as $table) {
        $maxIdStmt = $mysql->query("SELECT COALESCE(MAX(id), 0) AS max_id FROM $table");
        $lastId = (int) ($maxIdStmt->fetch()['max_id'] ?? 0);

        log_line("Starting sync for table '$table'. Last MySQL id = $lastId, batch size = $batchSize");

        $fetchSql = "
            SELECT *
            FROM $table
            WHERE id > :last_id
            ORDER BY id ASC
            LIMIT :limit
        ";

        $insertSql = "
            INSERT INTO $table (" . implode(", ", array_keys($row)) . ")
            VALUES (" . implode(", ", array_map(fn($col) => ":$col", array_keys($row))) . ")
            ON DUPLICATE KEY UPDATE " . implode(", ", array_map(fn($col) => "$col = VALUES($col)", array_keys($row))) . ";";
        $fetchStmt = $pgsql->prepare($fetchSql);
        $insertStmt = $mysql->prepare($insertSql);

        $totalInserted = 0;

        while (true) {
            $fetchStmt->bindValue(':last_id', $lastId, PDO::PARAM_INT);
            $fetchStmt->bindValue(':limit', $batchSize, PDO::PARAM_INT);
            $fetchStmt->execute();
            $rows = $fetchStmt->fetchAll();

            if ($rows === []) {
                break;
            }

            $mysql->beginTransaction();
            try {
                foreach ($rows as $row) {
                    $insertStmt->execute(array_combine(
                        array_map(fn($col) => ":$col", array_keys($row)),
                        array_values($row)
                    ));
                    $lastId = (int) $row['id'];
                    $totalInserted++;
                }
                $mysql->commit();
            } catch (Throwable $e) {
                $mysql->rollBack();
                throw $e;
            }

            log_line("Synced batch for table '$table'. New last id = $lastId, total upserts = $totalInserted");
        }

        log_line("Sync completed for table '$table'. Total upserts = $totalInserted");
    }

    log_line('Readings sync completed. Total upserts = ' . $totalInserted);
    exit(0);
} catch (Throwable $e) {
    fwrite(STDERR, 'Sync failed: ' . $e->getMessage() . PHP_EOL);
    exit(1);
}
