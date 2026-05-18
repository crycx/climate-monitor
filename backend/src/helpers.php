<?php
declare(strict_types=1);

function cm_load_env(string $path): void
{
    if (!is_file($path)) {
        return;
    }

    $lines = file($path, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES);
    if ($lines === false) {
        return;
    }

    foreach ($lines as $line) {
        $trimmed = trim($line);
        if ($trimmed === '' || str_starts_with($trimmed, '#') || !str_contains($trimmed, '=')) {
            continue;
        }

        [$key, $value] = explode('=', $trimmed, 2);
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

function cm_env(string $key, ?string $default = null): ?string
{
    $value = $_ENV[$key] ?? $_SERVER[$key] ?? getenv($key);
    if ($value === false || $value === null || $value === '') {
        return $default;
    }

    return (string) $value;
}

function cm_bool(?string $value, bool $default = false): bool
{
    if ($value === null || $value === '') {
        return $default;
    }

    return in_array(strtolower($value), ['1', 'true', 'yes', 'on'], true);
}

function cm_allowed_origin(string $origin, array $allowedOrigins): ?string
{
    foreach ($allowedOrigins as $allowed) {
        if ($allowed === '*' || $allowed === $origin) {
            return $origin;
        }
    }

    return null;
}

function cm_app_config(): array
{
    $frontendOriginsRaw = cm_env('FRONTEND_ORIGIN', 'http://localhost:4200') ?? 'http://localhost:4200';
    $frontendOrigins = array_values(array_filter(array_map('trim', explode(',', $frontendOriginsRaw))));

    return [
        'app_port' => (int) (cm_env('APP_PORT', '8000') ?? '8000'),
        'frontend_origins' => $frontendOrigins,
        'db_dsn' => cm_env('DATABASE_URL') ?? '',
        'db_host' => cm_env('DB_HOST', '127.0.0.1') ?? '127.0.0.1',
        'db_port' => (int) (cm_env('DB_PORT', '3306') ?? '3306'),
        'db_name' => cm_env('DB_NAME', 'climate_monitor') ?? 'climate_monitor',
        'db_user' => cm_env('DB_USER', 'root') ?? 'root',
        'db_password' => cm_env('DB_PASSWORD', '') ?? '',
        'secret_key' => cm_env('SECRET_KEY', 'dev-only-secret-change-me') ?? 'dev-only-secret-change-me',
        'session_cookie_secure' => cm_bool(cm_env('SESSION_COOKIE_SECURE'), false),
        'session_cookie_samesite' => cm_env('SESSION_COOKIE_SAMESITE', 'Lax') ?? 'Lax',
        'session_lifetime_hours' => (int) (cm_env('SESSION_LIFETIME_HOURS', '24') ?? '24'),
        'reset_token_ttl_minutes' => (int) (cm_env('RESET_TOKEN_TTL_MINUTES', '30') ?? '30'),
        'expose_reset_token' => cm_bool(cm_env('EXPOSE_RESET_TOKEN_IN_RESPONSE'), false),
        'frontend_reset_url' => cm_env('FRONTEND_RESET_URL', 'http://localhost:4200/reset-password') ?? 'http://localhost:4200/reset-password',
        'smtp_host' => cm_env('SMTP_HOST', '') ?? '',
        'smtp_port' => (int) (cm_env('SMTP_PORT', '587') ?? '587'),
        'smtp_username' => cm_env('SMTP_USERNAME', '') ?? '',
        'smtp_password' => cm_env('SMTP_PASSWORD', '') ?? '',
        'smtp_use_tls' => cm_bool(cm_env('SMTP_USE_TLS'), true),
        'mail_from' => cm_env('MAIL_FROM', 'no-reply@climate-monitor.local') ?? 'no-reply@climate-monitor.local',
        'forgot_password_rate_limit_count' => (int) (cm_env('FORGOT_PASSWORD_RATE_LIMIT_COUNT', '5') ?? '5'),
        'forgot_password_rate_limit_window_seconds' => (int) (cm_env('FORGOT_PASSWORD_RATE_LIMIT_WINDOW_SECONDS', '900') ?? '900'),
        'reset_password_rate_limit_count' => (int) (cm_env('RESET_PASSWORD_RATE_LIMIT_COUNT', '10') ?? '10'),
        'reset_password_rate_limit_window_seconds' => (int) (cm_env('RESET_PASSWORD_RATE_LIMIT_WINDOW_SECONDS', '900') ?? '900'),
        'self_registration_enabled' => cm_bool(cm_env('SELF_REGISTRATION_ENABLED'), true),
    ];
}

function cm_pdo(array $config): PDO
{
    if ($config['db_dsn'] !== '') {
        $dsn = $config['db_dsn'];
    } else {
        $dsn = sprintf(
            'mysql:host=%s;port=%d;dbname=%s;charset=utf8mb4',
            $config['db_host'],
            $config['db_port'],
            $config['db_name']
        );
    }

    $pdo = new PDO($dsn, $config['db_user'], $config['db_password'], [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
        PDO::ATTR_EMULATE_PREPARES => false,
    ]);

    $pdo->exec("SET time_zone = '+00:00'");
    return $pdo;
}

function cm_now_utc(): DateTimeImmutable
{
    return new DateTimeImmutable('now', new DateTimeZone('UTC'));
}

function cm_dt_from_db(?string $value): ?DateTimeImmutable
{
    if ($value === null || $value === '') {
        return null;
    }

    try {
        return new DateTimeImmutable($value, new DateTimeZone('UTC'));
    } catch (Throwable) {
        return null;
    }
}

function cm_iso_z(?string $value): ?string
{
    $dt = cm_dt_from_db($value);
    if ($dt === null) {
        return null;
    }

    return $dt->setTimezone(new DateTimeZone('UTC'))->format('Y-m-d\TH:i:s\Z');
}

function cm_parse_iso(string $value): ?DateTimeImmutable
{
    $normalized = trim($value);
    if ($normalized === '') {
        return null;
    }
    if (str_ends_with($normalized, 'Z')) {
        $normalized = substr($normalized, 0, -1) . '+00:00';
    }

    try {
        $dt = new DateTimeImmutable($normalized);
    } catch (Throwable) {
        return null;
    }

    return $dt->setTimezone(new DateTimeZone('UTC'));
}

function cm_json_body(): mixed
{
    $raw = file_get_contents('php://input');
    if ($raw === false || trim($raw) === '') {
        return [];
    }

    $decoded = json_decode($raw, true);
    if (json_last_error() !== JSON_ERROR_NONE) {
        return '__invalid_json__';
    }

    return $decoded;
}

function cm_header(string $name): ?string
{
    $serverKey = 'HTTP_' . strtoupper(str_replace('-', '_', $name));
    $value = $_SERVER[$serverKey] ?? null;
    if ($value === null || $value === '') {
        return null;
    }

    return (string) $value;
}

function cm_remote_identity(): string
{
    $forwardedFor = cm_header('X-Forwarded-For');
    if ($forwardedFor !== null) {
        return trim(explode(',', $forwardedFor)[0]);
    }

    return $_SERVER['REMOTE_ADDR'] ?? 'unknown';
}

function cm_hash_reset_token(string $token): string
{
    return hash('sha256', $token);
}

function cm_random_api_key(): string
{
    return bin2hex(random_bytes(32));
}

function cm_random_reset_token(): string
{
    return rtrim(strtr(base64_encode(random_bytes(32)), '+/', '-_'), '=');
}

function cm_time_to_mysql(DateTimeInterface $dt): string
{
    return $dt->setTimezone(new DateTimeZone('UTC'))->format('Y-m-d H:i:s');
}
