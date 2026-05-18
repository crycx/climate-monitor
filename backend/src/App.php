<?php
declare(strict_types=1);

final class ClimateMonitorApp
{
    private PDO $pdo;
    private array $config;
    private array $rateLimitStore = [];

    public function __construct(PDO $pdo, array $config)
    {
        $this->pdo = $pdo;
        $this->config = $config;

        date_default_timezone_set('UTC');
        ini_set('session.use_strict_mode', '1');
        ini_set('session.cookie_httponly', '1');
        ini_set('session.cookie_samesite', $this->config['session_cookie_samesite']);
        ini_set('session.cookie_secure', $this->config['session_cookie_secure'] ? '1' : '0');
        ini_set('session.gc_maxlifetime', (string) ($this->config['session_lifetime_hours'] * 3600));

        session_start();
    }

    public function handle(): void
    {
        $method = strtoupper($_SERVER['REQUEST_METHOD'] ?? 'GET');
        $path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';

        if ($path === '/openapi.yaml') {
            $this->serveOpenApi();
            return;
        }

        if ($path === '/swagger') {
            $this->serveSwaggerUi();
            return;
        }

        if ($path === '/' || $path === '') {
            $this->json(['message' => 'Climate Monitor API']);
            return;
        }

        if (!str_starts_with($path, '/api/')) {
            $this->error('Not found', 404);
            return;
        }

        try {
            $this->dispatch($method, $path);
        } catch (Throwable $e) {
            $this->error('Internal server error', 500, ['detail' => $e->getMessage()]);
        }
    }

    private function dispatch(string $method, string $path): void
    {
        if ($path === '/api/auth/register' && $method === 'POST') {
            $this->authRegister();
            return;
        }

        if ($path === '/api/auth/login' && $method === 'POST') {
            $this->authLogin();
            return;
        }

        if ($path === '/api/auth/forgot-password' && $method === 'POST') {
            $this->authForgotPassword();
            return;
        }

        if ($path === '/api/auth/reset-password' && $method === 'POST') {
            $this->authResetPassword();
            return;
        }

        if ($path === '/api/auth/logout' && $method === 'POST') {
            $this->authLogout();
            return;
        }

        if ($path === '/api/auth/me' && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->json($this->userToArray($user));
            }
            return;
        }

        if ($path === '/api/auth/change-password' && $method === 'POST') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->authChangePassword($user);
            }
            return;
        }

        if ($path === '/api/users' && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->usersList($user);
            }
            return;
        }

        if ($path === '/api/users' && $method === 'POST') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->usersCreate($user);
            }
            return;
        }

        if (preg_match('#^/api/users/(\d+)$#', $path, $matches)) {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->usersById($user, $method, (int) $matches[1]);
            }
            return;
        }

        if ($path === '/api/locations' && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->locationsList($user);
            }
            return;
        }

        if ($path === '/api/locations' && $method === 'POST') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->locationsCreate($user);
            }
            return;
        }

        if (preg_match('#^/api/locations/(\d+)$#', $path, $matches)) {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->locationsById($user, $method, (int) $matches[1]);
            }
            return;
        }

        if ($path === '/api/devices' && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->devicesList($user);
            }
            return;
        }

        if ($path === '/api/devices' && $method === 'POST') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->devicesCreate($user);
            }
            return;
        }

        if (preg_match('#^/api/devices/(\d+)/shares$#', $path, $matches)) {
            $user = $this->requireLogin();
            if ($user !== null) {
                if ($method === 'GET') {
                    $this->deviceSharesList($user, (int) $matches[1]);
                } elseif ($method === 'POST') {
                    $this->deviceSharesCreate($user, (int) $matches[1]);
                } else {
                    $this->error('Not found', 404);
                }
            }
            return;
        }

        if (preg_match('#^/api/devices/(\d+)/shares/(\d+)$#', $path, $matches) && $method === 'DELETE') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->deviceSharesDelete($user, (int) $matches[1], (int) $matches[2]);
            }
            return;
        }

        if (preg_match('#^/api/devices/(\d+)/shareable-users$#', $path, $matches) && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->deviceShareableUsers($user, (int) $matches[1]);
            }
            return;
        }

        if (preg_match('#^/api/devices/(\d+)/readings(?:\.csv)?$#', $path, $matches)) {
            $deviceId = (int) $matches[1];
            if (str_ends_with($path, '.csv')) {
                $user = $this->requireLogin();
                if ($user !== null) {
                    $this->deviceReadingsCsv($user, $deviceId);
                }
            } elseif ($method === 'GET') {
                $user = $this->requireLogin();
                if ($user !== null) {
                    $this->deviceReadings($user, $deviceId);
                }
            } elseif ($method === 'POST') {
                $device = $this->requireApiKey();
                if ($device !== null) {
                    $this->deviceReadingsCreateLegacy($device, $deviceId);
                }
            } else {
                $this->error('Not found', 404);
            }
            return;
        }

        if (preg_match('#^/api/devices/(\d+)$#', $path, $matches)) {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->devicesById($user, $method, (int) $matches[1]);
            }
            return;
        }

        if ($path === '/api/readings' && $method === 'POST') {
            $device = $this->requireApiKey();
            if ($device !== null) {
                $this->readingsCreate($device);
            }
            return;
        }

        if ($path === '/api/readings/latest' && $method === 'GET') {
            $user = $this->requireLogin();
            if ($user !== null) {
                $this->readingsLatest($user);
            }
            return;
        }

        if ($path === '/api/readings/latest_device' && $method === 'GET') {
            $device = $this->requireApiKey();
            if ($device !== null) {
                $this->readingsLatestDevice($device);
            }
            return;
        }

        $this->error('Not found', 404);
    }

    private function serveSwaggerUi(): void
    {
        header('Content-Type: text/html; charset=utf-8');
        echo <<<'HTML'
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Climate Monitor API</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
<div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script>
    SwaggerUIBundle({
        url: '/openapi.yaml',
        dom_id: '#swagger-ui',
        presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
        layout: 'BaseLayout',
        deepLinking: true,
    });
</script>
</body>
</html>
HTML;
    }

    private function serveOpenApi(): void
    {
        $file = dirname(__DIR__, 1) . '/swagger.yaml';
        if (!is_file($file)) {
            $this->error('OpenAPI spec not found', 404);
            return;
        }

        header('Content-Type: application/yaml; charset=utf-8');
        readfile($file);
    }

    private function requireLogin(): ?array
    {
        $userId = $_SESSION['user_id'] ?? null;
        if (!is_numeric($userId)) {
            $this->error('Authentication required', 401);
            return null;
        }

        $user = $this->fetchUserById((int) $userId);
        if ($user === null) {
            session_unset();
            session_destroy();
            $this->error('Authentication required', 401);
            return null;
        }

        return $user;
    }

    private function requireApiKey(): ?array
    {
        $apiKey = cm_header('X-API-Key');
        if ($apiKey === null || $apiKey === '') {
            $this->error('API key required', 401);
            return null;
        }

        $device = $this->fetchDeviceByApiKey($apiKey);
        if ($device === null || !(bool) $device['is_active']) {
            $this->error('Invalid or inactive API key', 401);
            return null;
        }

        $this->pdo->prepare('UPDATE devices SET last_seen = ?, updated_at = ? WHERE id = ?')
            ->execute([
                cm_time_to_mysql(cm_now_utc()),
                cm_time_to_mysql(cm_now_utc()),
                (int) $device['id'],
            ]);

        return $device;
    }

    private function authRegister(): void
    {
        if (!$this->config['self_registration_enabled']) {
            $this->error('Account creation is currently disabled', 403);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $username = trim((string) ($payload['username'] ?? ''));
        $email = strtolower(trim((string) ($payload['email'] ?? '')));
        $password = (string) ($payload['password'] ?? '');

        if ($username === '' || $email === '' || $password === '') {
            $this->error('username, email and password are required', 400);
            return;
        }
        if (!str_contains($email, '@')) {
            $this->error('email is not valid', 400);
            return;
        }

        if ($this->fetchUserByUsernameOrEmail($username, $email) !== null) {
            $this->error('username or email already exists', 409);
            return;
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('INSERT INTO users (username, email, password_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?)');
        $stmt->execute([$username, $email, password_hash($password, PASSWORD_DEFAULT), $now, $now]);
        $userId = (int) $this->pdo->lastInsertId();

        session_regenerate_id(true);
        $_SESSION['user_id'] = $userId;
        $_SESSION['permanent'] = true;

        $user = $this->fetchUserById($userId);
        $this->json($this->userToArray($user ?? []), 201);
    }

    private function authLogin(): void
    {
        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $username = trim((string) ($payload['username'] ?? ''));
        $password = (string) ($payload['password'] ?? '');
        if ($username === '' || $password === '') {
            $this->error('username and password are required', 400);
            return;
        }

        $user = $this->fetchUserByUsername($username);
        if ($user === null || !password_verify($password, (string) $user['password_hash'])) {
            $this->error('Invalid credentials', 401);
            return;
        }

        session_regenerate_id(true);
        $_SESSION['user_id'] = (int) $user['id'];
        $_SESSION['permanent'] = true;

        $this->json([
            'message' => 'Login successful',
            'user' => $this->userToArray($user),
        ]);
    }

    private function authForgotPassword(): void
    {
        $retryAfter = $this->checkRateLimit(
            'forgot-password',
            cm_remote_identity(),
            $this->config['forgot_password_rate_limit_count'],
            $this->config['forgot_password_rate_limit_window_seconds']
        );
        if ($retryAfter > 0) {
            $this->json([
                'error' => 'Too many forgot-password requests. Please try again later.',
                'retry_after_seconds' => $retryAfter,
            ], 429);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $email = strtolower(trim((string) ($payload['email'] ?? '')));
        if ($email === '') {
            $this->error('email is required', 400);
            return;
        }

        $user = $this->fetchUserByEmail($email);
        $resetToken = null;
        $emailSent = false;
        if ($user !== null) {
            $resetToken = $this->createResetToken((int) $user['id']);
            $emailSent = $this->sendResetPasswordEmail($email, $resetToken);
        }

        $response = ['message' => 'If that email exists, a reset link has been generated'];
        if ($user !== null && $this->config['expose_reset_token']) {
            $response['reset_token'] = $resetToken;
            $response['email_sent'] = $emailSent;
        }

        $this->json($response);
    }

    private function authResetPassword(): void
    {
        $retryAfter = $this->checkRateLimit(
            'reset-password',
            cm_remote_identity(),
            $this->config['reset_password_rate_limit_count'],
            $this->config['reset_password_rate_limit_window_seconds']
        );
        if ($retryAfter > 0) {
            $this->json([
                'error' => 'Too many reset-password requests. Please try again later.',
                'retry_after_seconds' => $retryAfter,
            ], 429);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $token = trim((string) ($payload['token'] ?? ''));
        $password = (string) ($payload['password'] ?? '');
        if ($token === '' || $password === '') {
            $this->error('token and password are required', 400);
            return;
        }

        $tokenHash = cm_hash_reset_token($token);
        $now = cm_now_utc();
        $stmt = $this->pdo->prepare('SELECT * FROM password_reset_tokens WHERE token_hash = ? AND used_at IS NULL LIMIT 1');
        $stmt->execute([$tokenHash]);
        $row = $stmt->fetch();
        if (!$row) {
            $this->error('Invalid or expired token', 400);
            return;
        }

        $expiresAt = cm_dt_from_db((string) $row['expires_at']);
        if ($expiresAt === null || $expiresAt <= $now) {
            $this->error('Invalid or expired token', 400);
            return;
        }

        $user = $this->fetchUserById((int) $row['user_id']);
        if ($user === null) {
            $this->error('Invalid or expired token', 400);
            return;
        }

        $updateUser = $this->pdo->prepare('UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?');
        $updateUser->execute([password_hash($password, PASSWORD_DEFAULT), cm_time_to_mysql($now), (int) $user['id']]);

        $updateToken = $this->pdo->prepare('UPDATE password_reset_tokens SET used_at = ? WHERE id = ?');
        $updateToken->execute([cm_time_to_mysql($now), (int) $row['id']]);

        $this->json(['message' => 'Password updated']);
    }

    private function authLogout(): void
    {
        $_SESSION = [];
        if (ini_get('session.use_cookies')) {
            $params = session_get_cookie_params();
            setcookie(session_name(), '', time() - 42000, $params['path'], $params['domain'], (bool) $params['secure'], (bool) $params['httponly']);
        }
        session_destroy();
        http_response_code(204);
    }

    private function authChangePassword(array $currentUser): void
    {
        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $currentPassword = (string) ($payload['current_password'] ?? '');
        $newPassword = (string) ($payload['new_password'] ?? '');
        if ($currentPassword === '' || $newPassword === '') {
            $this->error('current_password and new_password are required', 400);
            return;
        }

        if (!password_verify($currentPassword, (string) $currentUser['password_hash'])) {
            $this->error('Current password is incorrect', 401);
            return;
        }

        $stmt = $this->pdo->prepare('UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?');
        $stmt->execute([password_hash($newPassword, PASSWORD_DEFAULT), cm_time_to_mysql(cm_now_utc()), (int) $currentUser['id']]);
        $this->json(['message' => 'Password changed']);
    }

    private function usersList(array $currentUser): void
    {
        if ((bool) $currentUser['is_admin']) {
            $rows = $this->pdo->query('SELECT * FROM users ORDER BY id ASC')->fetchAll();
            $this->json(array_map([$this, 'userToArray'], $rows));
            return;
        }

        $this->json([$this->userToArray($currentUser)]);
    }

    private function usersCreate(array $currentUser): void
    {
        if (!(bool) $currentUser['is_admin']) {
            $this->error('Only admin can create users', 403);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $username = trim((string) ($payload['username'] ?? ''));
        $email = strtolower(trim((string) ($payload['email'] ?? '')));
        $isAdmin = $this->toBool($payload['is_admin'] ?? false);

        if ($username === '' || $email === '') {
            $this->error('username and email are required', 400);
            return;
        }
        if (!str_contains($email, '@')) {
            $this->error('email is not valid', 400);
            return;
        }

        if ($this->fetchUserByUsernameOrEmail($username, $email) !== null) {
            $this->error('username or email already exists', 409);
            return;
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $randomPassword = bin2hex(random_bytes(12));
        $stmt = $this->pdo->prepare('INSERT INTO users (username, email, password_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)');
        $stmt->execute([$username, $email, password_hash($randomPassword, PASSWORD_DEFAULT), $isAdmin ? 1 : 0, $now, $now]);
        $userId = (int) $this->pdo->lastInsertId();
        $user = $this->fetchUserById($userId);
        if ($user === null) {
            $this->error('User not created', 500);
            return;
        }

        $resetToken = $this->createResetToken($userId);
        $emailSent = $this->sendResetPasswordEmail($email, $resetToken);

        $response = [
            'message' => 'User created and password setup link generated',
            'user' => $this->userToArray($user),
            'email_sent' => $emailSent,
        ];
        if ($this->config['expose_reset_token']) {
            $response['reset_token'] = $resetToken;
        }

        $this->json($response, 201);
    }

    private function usersById(array $currentUser, string $method, int $userId): void
    {
        if (!$this->canManageUser($currentUser, $userId)) {
            $this->error('Forbidden', 403);
            return;
        }

        $user = $this->fetchUserById($userId);
        if ($user === null) {
            $this->error('User not found', 404);
            return;
        }

        if ($method === 'GET') {
            $this->json($this->userToArray($user));
            return;
        }

        if ($method === 'PATCH') {
            $payload = cm_json_body();
            if ($payload === '__invalid_json__') {
                $this->error('Invalid JSON body', 400);
                return;
            }

            $updates = [];
            if (array_key_exists('username', $payload)) {
                $username = trim((string) $payload['username']);
                if ($username === '') {
                    $this->error('username cannot be empty', 400);
                    return;
                }
                $updates['username'] = $username;
            }
            if (array_key_exists('email', $payload)) {
                $email = strtolower(trim((string) $payload['email']));
                if ($email === '' || !str_contains($email, '@')) {
                    $this->error('email is not valid', 400);
                    return;
                }
                $updates['email'] = $email;
            }
            if (array_key_exists('password', $payload)) {
                if (!(bool) $currentUser['is_admin']) {
                    $this->error('Only admin can reset password via this endpoint', 403);
                    return;
                }
                $password = (string) $payload['password'];
                if ($password === '') {
                    $this->error('password cannot be empty', 400);
                    return;
                }
                $updates['password_hash'] = password_hash($password, PASSWORD_DEFAULT);
            }
            if (array_key_exists('is_admin', $payload)) {
                if (!(bool) $currentUser['is_admin']) {
                    $this->error('Only admin can change admin privileges', 403);
                    return;
                }
                $updates['is_admin'] = $this->toBool($payload['is_admin']) ? 1 : 0;
            }

            if (!$updates) {
                $this->error('At least one of username, email, or password is required', 400);
                return;
            }

            if (isset($updates['username']) && $updates['username'] !== $user['username'] && $this->userNameExists($updates['username'], $userId)) {
                $this->error('username already exists', 409);
                return;
            }
            if (isset($updates['email']) && $updates['email'] !== $user['email'] && $this->userEmailExists($updates['email'], $userId)) {
                $this->error('email already exists', 409);
                return;
            }

            $updates['updated_at'] = cm_time_to_mysql(cm_now_utc());
            $sets = [];
            $values = [];
            foreach ($updates as $column => $value) {
                $sets[] = $column . ' = ?';
                $values[] = $value;
            }
            $values[] = $userId;
            $sql = 'UPDATE users SET ' . implode(', ', $sets) . ' WHERE id = ?';
            $this->pdo->prepare($sql)->execute($values);

            $updated = $this->fetchUserById($userId);
            $this->json($this->userToArray($updated ?? $user));
            return;
        }

        if ($method === 'DELETE') {
            $stmt = $this->pdo->prepare('DELETE FROM users WHERE id = ?');
            $stmt->execute([$userId]);
            if ((int) $currentUser['id'] === $userId) {
                $_SESSION = [];
                session_destroy();
            }
            http_response_code(204);
            return;
        }

        $this->error('Not found', 404);
    }

    private function locationsList(array $currentUser): void
    {
        if ((bool) $currentUser['is_admin']) {
            $rows = $this->pdo->query('SELECT * FROM locations ORDER BY id ASC')->fetchAll();
        } else {
            $stmt = $this->pdo->prepare('SELECT * FROM locations WHERE user_id = ? ORDER BY id ASC');
            $stmt->execute([(int) $currentUser['id']]);
            $rows = $stmt->fetchAll();
        }
        $this->json(array_map([$this, 'locationToArray'], $rows));
    }

    private function locationsCreate(array $currentUser): void
    {
        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $name = trim((string) ($payload['name'] ?? ''));
        if ($name === '') {
            $this->error('name is required', 400);
            return;
        }

        $userId = (int) $currentUser['id'];
        if ((bool) $currentUser['is_admin'] && array_key_exists('user_id', $payload)) {
            if (!is_numeric($payload['user_id'])) {
                $this->error('Invalid user_id', 400);
                return;
            }
            $userId = (int) $payload['user_id'];
            if ($this->fetchUserById($userId) === null) {
                $this->error('Invalid user_id', 400);
                return;
            }
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('INSERT INTO locations (user_id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?)');
        $stmt->execute([
            $userId,
            $name,
            isset($payload['description']) ? ($payload['description'] !== '' ? (string) $payload['description'] : null) : null,
            $now,
            $now,
        ]);

        $location = $this->fetchLocationById((int) $this->pdo->lastInsertId());
        $this->json($this->locationToArray($location ?? []), 201);
    }

    private function locationsById(array $currentUser, string $method, int $locationId): void
    {
        $location = $this->fetchLocationById($locationId);
        if ($location === null) {
            $this->error('Location not found', 404);
            return;
        }
        if (!$this->canAccessOwnedRow($currentUser, (int) $location['user_id'])) {
            $this->error('Access denied', 403);
            return;
        }

        if ($method === 'GET') {
            $this->json($this->locationToArray($location));
            return;
        }

        if ($method === 'PATCH') {
            $payload = cm_json_body();
            if ($payload === '__invalid_json__') {
                $this->error('Invalid JSON body', 400);
                return;
            }

            $updates = [];
            if (array_key_exists('name', $payload)) {
                $name = trim((string) $payload['name']);
                if ($name === '') {
                    $this->error('name cannot be empty', 400);
                    return;
                }
                $updates['name'] = $name;
            }
            if (array_key_exists('description', $payload)) {
                $updates['description'] = $payload['description'] !== '' ? (string) $payload['description'] : null;
            }
            if (!$updates) {
                $this->error('At least one field is required', 400);
                return;
            }
            $updates['updated_at'] = cm_time_to_mysql(cm_now_utc());

            $this->updateById('locations', $locationId, $updates);
            $updated = $this->fetchLocationById($locationId);
            $this->json($this->locationToArray($updated ?? $location));
            return;
        }

        if ($method === 'DELETE') {
            $this->pdo->prepare('DELETE FROM locations WHERE id = ?')->execute([$locationId]);
            http_response_code(204);
            return;
        }

        $this->error('Not found', 404);
    }

    private function devicesList(array $currentUser): void
    {
        if ((bool) $currentUser['is_admin']) {
            $rows = $this->pdo->query('SELECT * FROM devices ORDER BY id ASC')->fetchAll();
            $devices = array_map(fn(array $row) => $this->deviceToArray($row, false, false), $rows);
            $this->json($devices);
            return;
        }

        $ids = $this->accessibleDeviceIds((int) $currentUser['id']);
        if ($ids === []) {
            $this->json([]);
            return;
        }

        $placeholders = implode(',', array_fill(0, count($ids), '?'));
        $stmt = $this->pdo->prepare('SELECT * FROM devices WHERE id IN (' . $placeholders . ') ORDER BY id ASC');
        $stmt->execute(array_values($ids));
        $rows = $stmt->fetchAll();
        $devices = array_map(fn(array $row) => $this->deviceToArray($row, false, false), $rows);
        $this->json($devices);
    }

    private function devicesCreate(array $currentUser): void
    {
        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $name = trim((string) ($payload['name'] ?? ''));
        if ($name === '') {
            $this->error('name is required', 400);
            return;
        }

        $userId = (int) $currentUser['id'];
        if ((bool) $currentUser['is_admin'] && array_key_exists('user_id', $payload)) {
            if (!is_numeric($payload['user_id'])) {
                $this->error('Invalid user_id', 400);
                return;
            }
            $userId = (int) $payload['user_id'];
            if ($this->fetchUserById($userId) === null) {
                $this->error('Invalid user_id', 400);
                return;
            }
        }

        $locationId = null;
        if (array_key_exists('location_id', $payload) && $payload['location_id'] !== null && $payload['location_id'] !== '') {
            if (!is_numeric($payload['location_id'])) {
                $this->error('location_id must be an integer', 400);
                return;
            }
            $locationId = (int) $payload['location_id'];
            $location = $this->fetchLocationById($locationId);
            if ($location === null) {
                $this->error('Location not found', 404);
                return;
            }
            if (!(bool) $currentUser['is_admin'] && (int) $location['user_id'] !== $userId) {
                $this->error('Location does not belong to selected user', 400);
                return;
            }
            if ((bool) $currentUser['is_admin'] && !in_array((int) $location['user_id'], [0, $userId], true)) {
                $this->error('Location does not belong to selected user', 400);
                return;
            }
        }

        $apiKey = trim((string) ($payload['api_key'] ?? ''));
        if ($apiKey === '') {
            $apiKey = cm_random_api_key();
        }
        if (strlen($apiKey) !== 64) {
            $this->error('api_key must be 64 characters', 400);
            return;
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('INSERT INTO devices (user_id, location_id, name, device_type, api_key, is_active, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)');
        try {
            $stmt->execute([
                $userId,
                $locationId,
                $name,
                isset($payload['device_type']) ? (($payload['device_type'] !== '') ? (string) $payload['device_type'] : null) : null,
                $apiKey,
                array_key_exists('is_active', $payload) ? ($this->toBool($payload['is_active']) ? 1 : 0) : 1,
                $now,
                $now,
            ]);
        } catch (Throwable) {
            $this->error('api_key already exists', 409);
            return;
        }

        $device = $this->fetchDeviceById((int) $this->pdo->lastInsertId());
        if ($device === null) {
            $this->error('Device not created', 500);
            return;
        }
        $device['api_key'] = $apiKey;
        $device['can_manage'] = true;
        $this->json($this->deviceToArray($device, true, true), 201);
    }

    private function devicesById(array $currentUser, string $method, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canAccessDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        if ($method === 'GET') {
            $canManage = $this->canManageDevice($currentUser, $device);
            $device['can_manage'] = $canManage;
            $this->json($this->deviceToArray($device, $canManage, true));
            return;
        }

        if ($method === 'PATCH') {
            if (!$this->canManageDevice($currentUser, $device)) {
                $this->error('Access denied', 403);
                return;
            }

            $payload = cm_json_body();
            if ($payload === '__invalid_json__') {
                $this->error('Invalid JSON body', 400);
                return;
            }

            $updates = [];
            if (array_key_exists('name', $payload)) {
                $name = trim((string) $payload['name']);
                if ($name === '') {
                    $this->error('name cannot be empty', 400);
                    return;
                }
                $updates['name'] = $name;
            }
            if (array_key_exists('device_type', $payload)) {
                $updates['device_type'] = $payload['device_type'] !== '' ? (string) $payload['device_type'] : null;
            }
            if (array_key_exists('location_id', $payload)) {
                $locationId = $payload['location_id'];
                if ($locationId === null || $locationId === '') {
                    $updates['location_id'] = null;
                } else {
                    if (!is_numeric($locationId)) {
                        $this->error('location_id must be an integer', 400);
                        return;
                    }
                    $locationId = (int) $locationId;
                    $location = $this->fetchLocationById($locationId);
                    if ($location === null) {
                        $this->error('Location not found', 404);
                        return;
                    }
                    if ((int) $location['user_id'] !== (int) $device['user_id']) {
                        $this->error('Location does not belong to device owner', 400);
                        return;
                    }
                    $updates['location_id'] = $locationId;
                }
            }
            if (array_key_exists('is_active', $payload)) {
                $updates['is_active'] = $this->toBool($payload['is_active']) ? 1 : 0;
            }
            if (array_key_exists('api_key', $payload)) {
                $apiKey = trim((string) $payload['api_key']);
                if (strlen($apiKey) !== 64) {
                    $this->error('api_key must be 64 characters', 400);
                    return;
                }
                $updates['api_key'] = $apiKey;
            }
            if (!$updates) {
                $this->error('At least one field is required', 400);
                return;
            }
            $updates['updated_at'] = cm_time_to_mysql(cm_now_utc());
            try {
                $this->updateById('devices', $deviceId, $updates);
            } catch (Throwable) {
                $this->error('api_key already exists', 409);
                return;
            }
            $updated = $this->fetchDeviceById($deviceId);
            $canManage = $this->canManageDevice($currentUser, $updated ?? $device);
            $responseDevice = $updated ?? $device;
            $responseDevice['can_manage'] = $canManage;
            $this->json($this->deviceToArray($responseDevice, $canManage, true));
            return;
        }

        if ($method === 'DELETE') {
            if (!$this->canManageDevice($currentUser, $device)) {
                $this->error('Access denied', 403);
                return;
            }
            $this->pdo->prepare('DELETE FROM devices WHERE id = ?')->execute([$deviceId]);
            http_response_code(204);
            return;
        }

        $this->error('Not found', 404);
    }

    private function deviceSharesList(array $currentUser, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canManageDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        $stmt = $this->pdo->prepare('SELECT * FROM device_shares WHERE device_id = ? ORDER BY starts_at ASC, id ASC');
        $stmt->execute([$deviceId]);
        $rows = $stmt->fetchAll();
        $this->json(array_map(fn(array $row) => $this->sharePayload($row), $rows));
    }

    private function deviceShareableUsers(array $currentUser, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canManageDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        $stmt = $this->pdo->prepare('SELECT * FROM users WHERE id <> ? ORDER BY username ASC, id ASC');
        $stmt->execute([(int) $device['user_id']]);
        $this->json(array_map([$this, 'userToArray'], $stmt->fetchAll()));
    }

    private function deviceSharesCreate(array $currentUser, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canManageDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $sharedUser = null;
        if (isset($payload['shared_with_user_id']) && $payload['shared_with_user_id'] !== '') {
            if (!is_numeric($payload['shared_with_user_id'])) {
                $this->error('shared_with_user_id must be an integer', 400);
                return;
            }
            $sharedUser = $this->fetchUserById((int) $payload['shared_with_user_id']);
            if ($sharedUser === null) {
                $this->error('User not found for shared_with_user_id', 404);
                return;
            }
        } else {
            $email = strtolower(trim((string) ($payload['shared_with_email'] ?? '')));
            if ($email === '') {
                $this->error('shared_with_user_id or shared_with_email is required', 400);
                return;
            }
            $sharedUser = $this->fetchUserByEmail($email);
        }

        if ($sharedUser === null) {
            $this->error('User not found', 404);
            return;
        }
        if ((int) $sharedUser['id'] === (int) $device['user_id']) {
            $this->error('Device owner cannot be added as shared user', 400);
            return;
        }

        $startsAt = cm_parse_iso((string) ($payload['starts_at'] ?? ''));
        $endsAt = cm_parse_iso((string) ($payload['ends_at'] ?? ''));
        if ($startsAt === null || $endsAt === null) {
            $this->error('starts_at and ends_at are required', 400);
            return;
        }
        if ($startsAt >= $endsAt) {
            $this->error('starts_at must be earlier than ends_at', 400);
            return;
        }

        $stmt = $this->pdo->prepare('SELECT id FROM device_shares WHERE device_id = ? AND shared_with_user_id = ? AND starts_at <= ? AND (ends_at IS NULL OR ends_at >= ?) LIMIT 1');
        $stmt->execute([
            $deviceId,
            (int) $sharedUser['id'],
            cm_time_to_mysql($endsAt),
            cm_time_to_mysql($startsAt),
        ]);
        if ($stmt->fetch()) {
            $this->error('Overlapping share period already exists for this user', 409);
            return;
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('INSERT INTO device_shares (device_id, shared_with_user_id, starts_at, ends_at, created_at) VALUES (?, ?, ?, ?, ?)');
        $stmt->execute([
            $deviceId,
            (int) $sharedUser['id'],
            cm_time_to_mysql($startsAt),
            cm_time_to_mysql($endsAt),
            $now,
        ]);
        $share = $this->fetchDeviceShareById((int) $this->pdo->lastInsertId());
        $this->json($this->sharePayload($share ?? []), 201);
    }

    private function deviceSharesDelete(array $currentUser, int $deviceId, int $shareId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canManageDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        $share = $this->fetchDeviceShareById($shareId);
        if ($share === null || (int) $share['device_id'] !== $deviceId) {
            $this->error('Share not found', 404);
            return;
        }

        $this->pdo->prepare('DELETE FROM device_shares WHERE id = ?')->execute([$shareId]);
        http_response_code(204);
    }

    private function deviceReadings(array $currentUser, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canAccessDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        [$querySql, $params, $limit, $error] = $this->buildReadingsQuery('SELECT r.* FROM readings r', $deviceId);
        if ($error !== null) {
            $this->json($error['body'], $error['status']);
            return;
        }

        $stmt = $this->pdo->prepare($querySql . ' LIMIT ' . $limit);
        $stmt->execute($params);
        $rows = $stmt->fetchAll();
        $this->json(array_map([$this, 'readingToArray'], $rows));
    }

    private function deviceReadingsCsv(array $currentUser, int $deviceId): void
    {
        $device = $this->fetchDeviceById($deviceId);
        if ($device === null) {
            $this->error('Device not found', 404);
            return;
        }
        if (!$this->canAccessDevice($currentUser, $device)) {
            $this->error('Access denied', 403);
            return;
        }

        $fromTsRaw = $_GET['from_ts'] ?? null;
        $toTsRaw = $_GET['to_ts'] ?? null;
        $stmtSql = 'SELECT r.*, l.name AS location_name FROM readings r LEFT JOIN locations l ON l.id = r.location_id WHERE r.device_id = ?';
        $params = [$deviceId];
        if (is_string($fromTsRaw) && trim($fromTsRaw) !== '') {
            $fromTs = cm_parse_iso($fromTsRaw);
            if ($fromTs === null) {
                $this->json(['error' => 'Invalid from_ts; expected ISO-8601 timestamp'], 400);
                return;
            }
            $stmtSql .= ' AND r.timestamp >= ?';
            $params[] = cm_time_to_mysql($fromTs);
        }
        if (is_string($toTsRaw) && trim($toTsRaw) !== '') {
            $toTs = cm_parse_iso($toTsRaw);
            if ($toTs === null) {
                $this->json(['error' => 'Invalid to_ts; expected ISO-8601 timestamp'], 400);
                return;
            }
            $stmtSql .= ' AND r.timestamp <= ?';
            $params[] = cm_time_to_mysql($toTs);
        }
        $stmtSql .= ' ORDER BY r.timestamp ASC, r.id ASC';
        $stmt = $this->pdo->prepare($stmtSql);
        $stmt->execute($params);
        $rows = $stmt->fetchAll();

        $bySecond = [];
        foreach ($rows as $row) {
            if (!in_array($row['sensor_type'], ['temperature', 'humidity'], true) || empty($row['timestamp'])) {
                continue;
            }
            $ts = cm_dt_from_db((string) $row['timestamp']);
            if ($ts === null) {
                continue;
            }
            $ts = $ts->setTimezone(new DateTimeZone('UTC'))->setTime((int) $ts->format('H'), (int) $ts->format('i'), (int) $ts->format('s'));
            $key = (string) $ts->getTimestamp();
            if (!isset($bySecond[$key])) {
                $bySecond[$key] = [
                    'timestamp' => $ts->format('d.m.Y H:i:s'),
                    'location' => $row['location_name'] ?? '',
                    'temperature' => '',
                    'humidity' => '',
                ];
            }
            if (($row['location_name'] ?? '') !== '' && $bySecond[$key]['location'] === '') {
                $bySecond[$key]['location'] = (string) $row['location_name'];
            }
            $bySecond[$key][$row['sensor_type']] = $this->formatCsvNumber($row['value']);
        }

        $fp = fopen('php://temp', 'w+');
        if ($fp === false) {
            $this->error('Failed to create CSV output', 500);
            return;
        }
        fputcsv($fp, ['timestamp', 'location', 'temperature', 'humidity']);
        ksort($bySecond);
        foreach ($bySecond as $row) {
            fputcsv($fp, [$row['timestamp'], $row['location'], $row['temperature'], $row['humidity']]);
        }
        rewind($fp);
        $csv = stream_get_contents($fp) ?: '';
        fclose($fp);

        header('Content-Type: text/csv; charset=utf-8');
        header('Content-Disposition: attachment; filename="device-' . $deviceId . '-readings.csv"');
        echo $csv;
    }

    private function deviceReadingsCreateLegacy(array $device, int $deviceId): void
    {
        if ((int) $device['id'] !== $deviceId) {
            $this->error('Device ID does not match API key', 403);
            return;
        }

        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $items = $this->normalizeReadingsPayload($payload, false);
        if ($items === null) {
            return;
        }

        $saved = $this->saveReadings((int) $device['id'], $items, (int) $device['user_id'], (int) ($device['location_id'] ?? 0) ?: null);
        $this->json(['saved' => $saved], 201);
    }

    private function readingsCreate(array $device): void
    {
        $payload = cm_json_body();
        if ($payload === '__invalid_json__') {
            $this->error('Invalid JSON body', 400);
            return;
        }

        $items = $this->normalizeReadingsPayload($payload, true);
        if ($items === null) {
            return;
        }

        $saved = $this->saveReadings((int) $device['id'], $items, (int) $device['user_id'], (int) ($device['location_id'] ?? 0) ?: null);
        $this->json(['saved' => $saved], 201);
    }

    private function readingsLatest(array $currentUser): void
    {
        $ids = null;
        if (!(bool) $currentUser['is_admin']) {
            $ids = $this->accessibleDeviceIds((int) $currentUser['id']);
            if ($ids === []) {
                $this->json([]);
                return;
            }
        }

        [$sql, $params, $limit, $error] = $this->buildReadingsQuery('SELECT r.* FROM readings r', null, $ids);
        if ($error !== null) {
            $this->json($error['body'], $error['status']);
            return;
        }

        $stmt = $this->pdo->prepare($sql . ' LIMIT ' . $limit);
        $stmt->execute($params);
        $rows = $stmt->fetchAll();
        $this->json(array_map([$this, 'readingToArray'], $rows));
    }

    private function readingsLatestDevice(array $device): void
    {
        $sensorType = isset($_GET['sensor_type']) ? trim((string) $_GET['sensor_type']) : '';
        $sql = 'SELECT r.* FROM readings r WHERE r.device_id = ?';
        $params = [(int) $device['id']];
        if ($sensorType !== '') {
            $sql .= ' AND r.sensor_type = ?';
            $params[] = $sensorType;
        }
        $sql .= ' ORDER BY r.timestamp DESC, r.id DESC LIMIT 1';
        $stmt = $this->pdo->prepare($sql);
        $stmt->execute($params);
        $row = $stmt->fetch();
        if (!$row) {
            $this->error('No readings found', 404);
            return;
        }
        $this->json($this->readingToArray($row));
    }

    private function normalizeReadingsPayload(mixed $payload, bool $expectArray): ?array
    {
        if ($expectArray) {
            if (!is_array($payload) || array_is_list($payload) === false) {
                $this->error('Expected a JSON array of readings', 400);
                return null;
            }
            $items = $payload;
        } else {
            if (!is_array($payload) || array_is_list($payload) === true) {
                $this->error('Expected a JSON object reading', 400);
                return null;
            }
            $items = [$payload];
        }

        $normalized = [];
        foreach ($items as $item) {
            if (!is_array($item)) {
                $this->error('Each reading must be an object', 400);
                return null;
            }
            $sensorType = (string) ($item['sensor_type'] ?? '');
            $value = $item['value'] ?? null;
            $unit = (string) ($item['unit'] ?? '');
            $locationRaw = $item['location'] ?? null;
            if ($sensorType === '' || $value === null || $unit === '') {
                $this->error('Each reading must have sensor_type, value and unit', 400);
                return null;
            }
            $normalized[] = [
                'sensor_type' => $sensorType,
                'value' => $value,
                'unit' => $unit,
                'location_name' => is_string($locationRaw) ? trim($locationRaw) : null,
            ];
        }

        return $normalized;
    }

    private function saveReadings(int $deviceId, array $readingsPayload, int $ownerUserId, ?int $deviceLocationId): int
    {
        $saved = 0;
        $latestLocationId = $deviceLocationId;
        $now = cm_time_to_mysql(cm_now_utc());

        $this->pdo->beginTransaction();
        try {
            foreach ($readingsPayload as $item) {
                $locationId = $deviceLocationId;
                if (!empty($item['location_name'])) {
                    $locationId = $this->resolveOrCreateLocation($ownerUserId, $item['location_name']);
                    $latestLocationId = $locationId;
                }

                $stmt = $this->pdo->prepare('INSERT INTO readings (device_id, location_id, sensor_type, value, unit, timestamp) VALUES (?, ?, ?, ?, ?, ?)');
                $stmt->execute([
                    $deviceId,
                    $locationId,
                    $item['sensor_type'],
                    $item['value'],
                    $item['unit'],
                    $now,
                ]);
                $saved++;
            }

            if ($latestLocationId !== $deviceLocationId) {
                $this->pdo->prepare('UPDATE devices SET location_id = ?, updated_at = ? WHERE id = ?')->execute([
                    $latestLocationId,
                    $now,
                    $deviceId,
                ]);
            }

            $this->pdo->commit();
            return $saved;
        } catch (Throwable $e) {
            $this->pdo->rollBack();
            throw $e;
        }
    }

    private function resolveOrCreateLocation(int $userId, string $locationName): ?int
    {
        $normalized = mb_strtolower(trim($locationName));
        if ($normalized === '') {
            return null;
        }

        $stmt = $this->pdo->prepare('SELECT id FROM locations WHERE user_id = ? AND LOWER(name) = ? LIMIT 1');
        $stmt->execute([$userId, $normalized]);
        $row = $stmt->fetch();
        if ($row) {
            return (int) $row['id'];
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('INSERT INTO locations (user_id, name, description, created_at, updated_at) VALUES (?, ?, NULL, ?, ?)');
        $stmt->execute([$userId, $locationName, $now, $now]);
        return (int) $this->pdo->lastInsertId();
    }

    private function buildReadingsQuery(string $baseSql, ?int $deviceId = null, ?array $deviceIds = null): array
    {
        $sql = $baseSql;
        $params = [];
        $limit = (int) ($_GET['limit'] ?? 50000);
        if ($limit < 1) {
            return ['', [], 0, ['body' => ['error' => 'limit must be a positive integer'], 'status' => 400]];
        }
        $limit = min($limit, 50000);

        $order = strtolower(trim((string) ($_GET['order'] ?? 'desc')));
        if (!in_array($order, ['asc', 'desc'], true)) {
            return ['', [], 0, ['body' => ['error' => 'order must be asc or desc'], 'status' => 400]];
        }

        if ($deviceId !== null) {
            $sql .= ' WHERE r.device_id = ?';
            $params[] = $deviceId;
        } elseif ($deviceIds !== null) {
            $placeholders = implode(',', array_fill(0, count($deviceIds), '?'));
            $sql .= ' WHERE r.device_id IN (' . $placeholders . ')';
            $params = array_merge($params, array_values($deviceIds));
        } else {
            $sql .= ' WHERE 1=1';
        }

        if (isset($_GET['location_id']) && $_GET['location_id'] !== '') {
            if (!is_numeric($_GET['location_id'])) {
                return ['', [], 0, ['body' => ['error' => 'location_id must be an integer'], 'status' => 400]];
            }
            $sql .= ' AND r.location_id = ?';
            $params[] = (int) $_GET['location_id'];
        }

        if (!empty($_GET['sensor_type'])) {
            $sql .= ' AND r.sensor_type = ?';
            $params[] = trim((string) $_GET['sensor_type']);
        }

        if (isset($_GET['after_id']) && $_GET['after_id'] !== '') {
            if (!is_numeric($_GET['after_id'])) {
                return ['', [], 0, ['body' => ['error' => 'after_id must be an integer'], 'status' => 400]];
            }
            $sql .= ' AND r.id > ?';
            $params[] = (int) $_GET['after_id'];
        }

        if (!empty($_GET['after_ts'])) {
            $dt = cm_parse_iso((string) $_GET['after_ts']);
            if ($dt === null) {
                return ['', [], 0, ['body' => ['error' => 'Invalid after_ts; expected ISO-8601 timestamp'], 'status' => 400]];
            }
            $sql .= ' AND r.timestamp > ?';
            $params[] = cm_time_to_mysql($dt);
        }

        $period = trim((string) ($_GET['period'] ?? ''));
        $fromTsRaw = trim((string) ($_GET['from_ts'] ?? ''));
        $toTsRaw = trim((string) ($_GET['to_ts'] ?? ''));
        if ($period !== '' && ($fromTsRaw !== '' || $toTsRaw !== '')) {
            return ['', [], 0, ['body' => ['error' => 'period cannot be combined with from_ts/to_ts'], 'status' => 400]];
        }

        if ($period !== '') {
            $utcNow = cm_now_utc();
            $todayStart = $utcNow->setTime(0, 0, 0);
            if ($period === 'today') {
                $sql .= ' AND r.timestamp >= ?';
                $params[] = cm_time_to_mysql($todayStart);
            } elseif ($period === 'yesterday') {
                $sql .= ' AND r.timestamp >= ? AND r.timestamp < ?';
                $params[] = cm_time_to_mysql($todayStart->sub(new DateInterval('P1D')));
                $params[] = cm_time_to_mysql($todayStart);
            } elseif ($period === 'last_24h') {
                $sql .= ' AND r.timestamp >= ?';
                $params[] = cm_time_to_mysql($utcNow->sub(new DateInterval('PT24H')));
            } else {
                return ['', [], 0, ['body' => ['error' => 'Invalid period; use today, yesterday, or last_24h'], 'status' => 400]];
            }
        } else {
            if ($fromTsRaw !== '') {
                $dt = cm_parse_iso($fromTsRaw);
                if ($dt === null) {
                    return ['', [], 0, ['body' => ['error' => 'Invalid from_ts; expected ISO-8601 timestamp'], 'status' => 400]];
                }
                $sql .= ' AND r.timestamp >= ?';
                $params[] = cm_time_to_mysql($dt);
            }
            if ($toTsRaw !== '') {
                $dt = cm_parse_iso($toTsRaw);
                if ($dt === null) {
                    return ['', [], 0, ['body' => ['error' => 'Invalid to_ts; expected ISO-8601 timestamp'], 'status' => 400]];
                }
                $sql .= ' AND r.timestamp <= ?';
                $params[] = cm_time_to_mysql($dt);
            }
        }

        $sql .= ' ORDER BY r.timestamp ' . strtoupper($order) . ', r.id ' . strtoupper($order);
        return [$sql, $params, $limit, null];
    }

    private function createResetToken(int $userId): string
    {
        $now = cm_now_utc();
        $stmt = $this->pdo->prepare('UPDATE password_reset_tokens SET used_at = ? WHERE user_id = ? AND used_at IS NULL');
        $stmt->execute([cm_time_to_mysql($now), $userId]);

        $token = cm_random_reset_token();
        $stmt = $this->pdo->prepare('INSERT INTO password_reset_tokens (user_id, token_hash, expires_at, used_at, created_at) VALUES (?, ?, ?, NULL, ?)');
        $stmt->execute([
            $userId,
            cm_hash_reset_token($token),
            cm_time_to_mysql($now->add(new DateInterval('PT' . $this->config['reset_token_ttl_minutes'] . 'M'))),
            cm_time_to_mysql($now),
        ]);

        return $token;
    }

    private function sendResetPasswordEmail(string $toEmail, string $token): bool
    {
        if ($this->config['smtp_host'] === '') {
            return false;
        }

        $subject = 'Climate Monitor password reset';
        $link = $this->buildResetLink($token);
        $message = "You requested a password reset.\n\nOpen this link to set a new password:\n" . $link . "\n\nThis link expires in " . $this->config['reset_token_ttl_minutes'] . " minutes.\nIf you did not request this, you can ignore this email.\n";
        $headers = 'From: ' . $this->config['mail_from'];

        if (function_exists('mail')) {
            return @mail($toEmail, $subject, $message, $headers);
        }

        return false;
    }

    private function buildResetLink(string $token): string
    {
        $base = $this->config['frontend_reset_url'];
        $join = str_contains($base, '?') ? '&' : '?';
        return $base . $join . http_build_query(['token' => $token]);
    }

    private function checkRateLimit(string $action, string $identity, int $limitCount, int $windowSeconds): int
    {
        $now = microtime(true);
        $windowStart = $now - $windowSeconds;
        $key = $action . ':' . $identity;
        $hits = $this->rateLimitStore[$key] ?? [];
        $hits = array_values(array_filter($hits, fn(float $hit) => $hit >= $windowStart));

        if (count($hits) >= $limitCount) {
            $retryAfter = (int) ceil(($hits[0] + $windowSeconds) - $now);
            $this->rateLimitStore[$key] = $hits;
            return max($retryAfter, 1);
        }

        $hits[] = $now;
        $this->rateLimitStore[$key] = $hits;
        return 0;
    }

    private function json(mixed $data, int $status = 200): void
    {
        http_response_code($status);
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode($data, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
    }

    private function error(string $message, int $status = 400, array $extra = []): void
    {
        $this->json(array_merge(['error' => $message], $extra), $status);
    }

    private function updateById(string $table, int $id, array $updates): void
    {
        $sets = [];
        $values = [];
        foreach ($updates as $column => $value) {
            $sets[] = $column . ' = ?';
            $values[] = $value;
        }
        $values[] = $id;
        $sql = 'UPDATE ' . $table . ' SET ' . implode(', ', $sets) . ' WHERE id = ?';
        $this->pdo->prepare($sql)->execute($values);
    }

    private function fetchUserById(int $id): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM users WHERE id = ? LIMIT 1');
        $stmt->execute([$id]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchUserByUsername(string $username): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM users WHERE username = ? LIMIT 1');
        $stmt->execute([$username]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchUserByEmail(string $email): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM users WHERE email = ? LIMIT 1');
        $stmt->execute([$email]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchUserByUsernameOrEmail(string $username, string $email): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM users WHERE username = ? OR email = ? LIMIT 1');
        $stmt->execute([$username, $email]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchLocationById(int $id): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM locations WHERE id = ? LIMIT 1');
        $stmt->execute([$id]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchDeviceById(int $id): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM devices WHERE id = ? LIMIT 1');
        $stmt->execute([$id]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchDeviceByApiKey(string $apiKey): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM devices WHERE api_key = ? LIMIT 1');
        $stmt->execute([$apiKey]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchDeviceShareById(int $id): ?array
    {
        $stmt = $this->pdo->prepare('SELECT * FROM device_shares WHERE id = ? LIMIT 1');
        $stmt->execute([$id]);
        $row = $stmt->fetch();
        return $row ?: null;
    }

    private function fetchDeviceShareUser(int $userId): ?array
    {
        return $this->fetchUserById($userId);
    }

    private function canAccessOwnedRow(array $currentUser, int $ownerUserId): bool
    {
        return (bool) $currentUser['is_admin'] || (int) $currentUser['id'] === $ownerUserId;
    }

    private function canManageUser(array $currentUser, int $userId): bool
    {
        return (bool) $currentUser['is_admin'] || (int) $currentUser['id'] === $userId;
    }

    private function canManageDevice(array $currentUser, array $device): bool
    {
        return (bool) $currentUser['is_admin'] || (int) $device['user_id'] === (int) $currentUser['id'];
    }

    private function canAccessDevice(array $currentUser, array $device): bool
    {
        if ($this->canManageDevice($currentUser, $device)) {
            return true;
        }

        $now = cm_time_to_mysql(cm_now_utc());
        $stmt = $this->pdo->prepare('SELECT id FROM device_shares WHERE device_id = ? AND shared_with_user_id = ? AND starts_at <= ? AND (ends_at IS NULL OR ends_at >= ?) LIMIT 1');
        $stmt->execute([
            (int) $device['id'],
            (int) $currentUser['id'],
            $now,
            $now,
        ]);

        return (bool) $stmt->fetch();
    }

    private function accessibleDeviceIds(int $userId): array
    {
        $stmt = $this->pdo->prepare('SELECT id FROM devices WHERE user_id = ?');
        $stmt->execute([$userId]);
        $ids = array_map(fn(array $row) => (int) $row['id'], $stmt->fetchAll());

        $stmt = $this->pdo->prepare('SELECT device_id FROM device_shares WHERE shared_with_user_id = ? AND starts_at <= ? AND (ends_at IS NULL OR ends_at >= ?)');
        $now = cm_time_to_mysql(cm_now_utc());
        $stmt->execute([$userId, $now, $now]);
        foreach ($stmt->fetchAll() as $row) {
            $ids[] = (int) $row['device_id'];
        }

        return array_values(array_unique($ids));
    }

    private function userNameExists(string $username, int $excludeUserId): bool
    {
        $stmt = $this->pdo->prepare('SELECT id FROM users WHERE username = ? AND id <> ? LIMIT 1');
        $stmt->execute([$username, $excludeUserId]);
        return (bool) $stmt->fetch();
    }

    private function userEmailExists(string $email, int $excludeUserId): bool
    {
        $stmt = $this->pdo->prepare('SELECT id FROM users WHERE email = ? AND id <> ? LIMIT 1');
        $stmt->execute([$email, $excludeUserId]);
        return (bool) $stmt->fetch();
    }

    private function sharePayload(array $share): array
    {
        $sharedUser = isset($share['shared_with_user_id']) ? $this->fetchDeviceShareUser((int) $share['shared_with_user_id']) : null;
        $payload = $this->deviceShareToArray($share);
        $payload['shared_with'] = $sharedUser ? $this->userToArray($sharedUser) : null;
        return $payload;
    }

    private function formatCsvNumber(mixed $value): string
    {
        if (!is_numeric($value)) {
            return '';
        }
        $formatted = rtrim(rtrim(number_format((float) $value, 4, '.', ''), '0'), '.');
        return $formatted !== '' ? $formatted : '0';
    }

    private function toBool(mixed $value): bool
    {
        if (is_bool($value)) {
            return $value;
        }
        if (is_numeric($value)) {
            return (int) $value !== 0;
        }
        return in_array(strtolower((string) $value), ['1', 'true', 'yes', 'on'], true);
    }

    private function userToArray(array $user): array
    {
        return [
            'id' => (int) $user['id'],
            'username' => $user['username'],
            'email' => $user['email'],
            'is_admin' => (bool) $user['is_admin'],
            'created_at' => cm_iso_z($user['created_at'] ?? null),
        ];
    }

    private function locationToArray(array $location): array
    {
        return [
            'id' => (int) $location['id'],
            'user_id' => $location['user_id'] !== null ? (int) $location['user_id'] : null,
            'name' => $location['name'],
            'description' => $location['description'],
            'created_at' => cm_iso_z($location['created_at'] ?? null),
            'updated_at' => cm_iso_z($location['updated_at'] ?? null),
        ];
    }

    private function deviceToArray(array $device, bool $includeApiKey = false, bool $includeManage = false): array
    {
        $result = [
            'id' => (int) $device['id'],
            'user_id' => (int) $device['user_id'],
            'location_id' => $device['location_id'] !== null ? (int) $device['location_id'] : null,
            'name' => $device['name'],
            'device_type' => $device['device_type'],
            'is_active' => (bool) $device['is_active'],
            'last_seen' => cm_iso_z($device['last_seen'] ?? null),
            'created_at' => cm_iso_z($device['created_at'] ?? null),
        ];
        if ($includeApiKey && isset($device['api_key'])) {
            $result['api_key'] = $device['api_key'];
        }
        if ($includeManage) {
            $result['can_manage'] = (bool) ($device['can_manage'] ?? false);
        }
        return $result;
    }

    private function deviceShareToArray(array $share): array
    {
        return [
            'id' => (int) $share['id'],
            'device_id' => (int) $share['device_id'],
            'shared_with_user_id' => (int) $share['shared_with_user_id'],
            'starts_at' => cm_iso_z($share['starts_at'] ?? null),
            'ends_at' => cm_iso_z($share['ends_at'] ?? null),
            'created_at' => cm_iso_z($share['created_at'] ?? null),
        ];
    }

    private function readingToArray(array $reading): array
    {
        return [
            'id' => (int) $reading['id'],
            'device_id' => (int) $reading['device_id'],
            'location_id' => $reading['location_id'] !== null ? (int) $reading['location_id'] : null,
            'location_name' => $reading['location_name'] ?? null,
            'sensor_type' => $reading['sensor_type'],
            'value' => (float) $reading['value'],
            'unit' => $reading['unit'],
            'timestamp' => cm_iso_z($reading['timestamp'] ?? null),
        ];
    }
}
