# vsioncode Windows installer
# Uso: iwr -useb https://raw.githubusercontent.com/jchacon1808/vsioncode/main/install.ps1 | iex

$repo = "jchacon1808/vsioncode"
$tag  = "v0.1.0"
$zip  = "vsioncode-windows.zip"
$url  = "https://github.com/$repo/releases/download/$tag/$zip"
$dest = "$env:USERPROFILE\vsioncode"
$bin  = "$dest\vsioncode.exe"

Write-Host "Descargando $zip desde $url ..."
Invoke-WebRequest -Uri $url -OutFile "$env:TEMP\$zip" -UseBasicParsing

Write-Host "Extrayendo en $dest ..."
Expand-Archive -Path "$env:TEMP\$zip" -DestinationPath $dest -Force

# Añadir al PATH de usuario (persistente)
$oldPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($oldPath -notlike "*$dest*") {
    $newPath = "$oldPath;$dest"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "Añadido $dest al PATH de usuario (requiere reiniciar terminal)"
} else {
    Write-Host "$dest ya está en PATH"
}

Write-Host "Listo. Ejecuta 'vsioncode' en una NUEVA terminal."
