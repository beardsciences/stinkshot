# Create the Forgejo release for a tag and upload the dist files to it.
# Asks for a Forgejo access token (scope: write:repository) and keeps it in memory only.
#
#   powershell -ExecutionPolicy Bypass -File scripts\publish-forgejo.ps1 -Version 0.1.1
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$DistDir = "C:\dev\stinkshot-bench\dist\$Version",
    [string]$Repo = 'christian/stinkshot',
    [string]$Server = 'https://code.mccullough.dev'
)
$ErrorActionPreference = 'Stop'
$tag = "v$Version"
$notes = Join-Path $DistDir 'RELEASE_NOTES.md'
if (-not (Test-Path $notes)) { throw "No release in $DistDir (run scripts\release.ps1 first)." }

$secure = Read-Host "Forgejo token for $Server (Settings > Applications, scope write:repository)" -AsSecureString
$token = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
$auth = "Authorization: token $token"
$api = "$Server/api/v1/repos/$Repo"

# Reuse the release if the tag already has one, otherwise create it.
$existing = curl.exe -s -H $auth "$api/releases/tags/$tag" | ConvertFrom-Json
if ($existing.id) {
    $id = $existing.id
    Write-Host "Release $tag exists (id $id); uploading missing files."
} else {
    $body = @{ tag_name = $tag; name = "Stinkshot $Version"; body = [IO.File]::ReadAllText($notes); draft = $false; prerelease = $false } | ConvertTo-Json
    $tmp = New-TemporaryFile
    [IO.File]::WriteAllText($tmp, $body, (New-Object Text.UTF8Encoding $false))
    $created = curl.exe -s -H $auth -H 'Content-Type: application/json' --data-binary "@$tmp" "$api/releases" | ConvertFrom-Json
    Remove-Item $tmp
    if (-not $created.id) { throw "Creating the release failed: $($created | ConvertTo-Json -Compress)" }
    $id = $created.id
    Write-Host "Created release $tag (id $id)."
}

$have = @((curl.exe -s -H $auth "$api/releases/$id/assets" | ConvertFrom-Json) | ForEach-Object { $_.name })
Get-ChildItem -File $DistDir | Where-Object { $_.Name -ne 'RELEASE_NOTES.md' } | ForEach-Object {
    if ($have -contains $_.Name) { Write-Host "  skip $($_.Name) (already uploaded)"; return }
    Write-Host "  uploading $($_.Name) ($([math]::Round($_.Length / 1MB, 1)) MB)..."
    $raw = curl.exe -s -S -H $auth -F "attachment=@$($_.FullName)" "$api/releases/$id/assets?name=$($_.Name)" 2>&1 | Out-String
    $r = $null
    try { $r = $raw | ConvertFrom-Json } catch {}
    if ($r.name) { Write-Host "  uploaded $($_.Name)" } else { Write-Warning "upload failed: $($_.Name): $($raw.Trim())" }
}
$token = $null
Write-Host "Done: $Server/$Repo/releases/tag/$tag"
