
function ParsePrismWorkspace {
    $metadata = cargo metadata --no-deps --offline | ConvertFrom-Json
    $env:PRISM_WORKSPACE = $metadata.workspace_root
    $env:RELEASE_VERSION = $metadata.packages | Where-Object { $_.name -eq "zed" } | Select-Object -ExpandProperty version
}
