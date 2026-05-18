<?php
declare(strict_types=1);

require_once __DIR__ . '/src/helpers.php';
require_once __DIR__ . '/src/App.php';

cm_load_env(__DIR__ . '/.env');
cm_load_env(__DIR__ . '/../.env');

$origin = $_SERVER['HTTP_ORIGIN'] ?? '';
$config = cm_app_config();

if ($origin !== '') {
    $allowed = cm_allowed_origin($origin, $config['frontend_origins']);
    if ($allowed !== null) {
        header('Access-Control-Allow-Origin: ' . $allowed);
        header('Access-Control-Allow-Credentials: true');
        header('Vary: Origin');
        header('Access-Control-Allow-Headers: Content-Type, X-API-Key, X-Requested-With');
        header('Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS');
    }
}

if (($_SERVER['REQUEST_METHOD'] ?? 'GET') === 'OPTIONS') {
    http_response_code(204);
    exit;
}

header('Content-Type: application/json; charset=utf-8');

$app = new ClimateMonitorApp(cm_pdo($config), $config);
$app->handle();
