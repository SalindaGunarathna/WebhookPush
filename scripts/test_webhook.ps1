<#  #>param(
  [Parameter(Mandatory = $true)]
  [string]$Uuid,
  [string]$BaseUrl = "http://localhost:3000",
  [int]$Bytes = 1024,
  [int]$Count = 1
)

function New-Payload([int]$size) {
  $blob = "a" * $size
  $timestamp = (Get-Date).ToString("o")
  return "{""timestamp"":""$timestamp"",""payload"":""$blob""}"
}

$url = "$BaseUrl/hook/$Uuid"

for ($i = 1; $i -le $Count; $i++) {
  $body = New-Payload -size $Bytes
  try {
    $response = Invoke-WebRequest -Uri $url -Method Post -ContentType "application/json" -Body $body -SkipHttpErrorCheck
    $status = $response.StatusCode
  } catch {
    $status = $_.Exception.Response.StatusCode.Value__
  }
  Write-Host "Sent payload size ~${Bytes} bytes to $url => HTTP $status"
}
