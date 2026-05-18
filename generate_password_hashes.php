<?php
// Genereeri bcrypt hashid ja SQL UPDATE käsud

$users = [
    'kristjan' => 'Kristjan123!',
    'arjom' => 'Arjom456!',
    'andres' => 'Andres789!'
];

echo "-- Kopeeri need UPDATE käsud phpMyAdmin-i SQL konsooli\n\n";

foreach ($users as $username => $password) {
    $hash = password_hash($password, PASSWORD_DEFAULT);
    echo "-- Kasutaja: $username, parool: $password\n";
    echo "UPDATE users SET password_hash = '$hash' WHERE username = '$username';\n\n";
}
?>
