<#
.SYNOPSIS
    Runs warden's full local check suite unattended and logs every step.

.DESCRIPTION
    Runs, in order, continuing after any failure:
      fmt --check, clippy -D warnings, cargo doc, cargo test, cargo test -- --ignored,
      the SpiceDB difftests (skipped if SpiceDB is not reachable), maturin develop, pytest,
      mypy --strict, each fuzz target for -FuzzSeconds, cargo-mutants on the evaluator and
      the schema validator, and the benchmarks (Rust, Python, SpiceDB if Docker is
      available), then benches/results.py.

    Each step's output goes to logs/overnight-<timestamp>/NN-<step>.log. SUMMARY.txt gets one
    line per step (PASS / FAIL / SKIP, duration, exit code), written as each step finishes,
    so a run cut short still leaves a summary.

    Windows PowerShell 5.1 or later. Expects the repo's .venv (see README) and, for the
    fuzz steps, a nightly toolchain and cargo-fuzz; cargo-mutants for the mutants steps.

.PARAMETER FuzzSeconds
    Seconds per fuzz target (default 600, the minimum docs/07-testing.md section 5 asks for).

.PARAMETER MutantsJobs
    Parallel cargo-mutants jobs (default 2). Each job builds in its own copy of the tree.

.PARAMETER SpiceDbUrl
    SpiceDB HTTP gateway for the difftests. Defaults to $env:WARDEN_SPICEDB_URL, else
    http://127.0.0.1:8443. With no preshared key in $env:WARDEN_SPICEDB_KEY, the key
    "overnight" is used, which suits `spicedb serve-testing`.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\overnight.ps1
#>
[CmdletBinding()]
param(
    [int]$FuzzSeconds = 600,
    [int]$MutantsJobs = 2,
    [string]$SpiceDbUrl = $(if ($env:WARDEN_SPICEDB_URL) { $env:WARDEN_SPICEDB_URL } else { 'http://127.0.0.1:8443' })
)

$ErrorActionPreference = 'Continue'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$LogDir = Join-Path $Repo "logs\overnight-$Stamp"
New-Item -ItemType Directory -Force $LogDir | Out-Null
$Summary = Join-Path $LogDir 'SUMMARY.txt'
$Results = New-Object System.Collections.ArrayList
$script:Index = 0

# ---- environment ----

$Venv = Join-Path $Repo '.venv'
$Python = Join-Path $Venv 'Scripts\python.exe'
$env:Path = "$Venv\Scripts;$env:USERPROFILE\.cargo\bin;$env:Path"
$env:VIRTUAL_ENV = $Venv
$env:CARGO_TERM_COLOR = 'never'
$env:RUST_BACKTRACE = '1'

# cargo-fuzz links AddressSanitizer; on Windows the targets need MSVC's runtime DLL on PATH.
$AsanDir = $null
$VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $VsWhere) {
    foreach ($vs in & $VsWhere -all -products * -property installationPath) {
        $dll = Get-ChildItem (Join-Path $vs 'VC\Tools\MSVC\*\bin\Hostx64\x64\clang_rt.asan_dynamic-x86_64.dll') -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1
        if ($dll) { $AsanDir = $dll.DirectoryName; break }
    }
}
if ($AsanDir) { $env:Path = "$AsanDir;$env:Path" }

function Write-Summary([string]$Line) {
    Add-Content -Path $Summary -Value $Line -Encoding ascii
}

function Format-Duration([TimeSpan]$t) {
    '{0:00}:{1:00}:{2:00}' -f [int][Math]::Floor($t.TotalHours), $t.Minutes, $t.Seconds
}

function Add-Result([string]$Name, [string]$Status, [TimeSpan]$Took, [string]$Exit, [string]$Log, [string]$Note) {
    $r = [pscustomobject]@{ Name = $Name; Status = $Status; Took = $Took; Exit = $Exit; Log = $Log; Note = $Note }
    [void]$Results.Add($r)
    $line = '{0,-4} {1,-38} {2,9}  exit {3,-4} {4}' -f $Status, $Name, (Format-Duration $Took), $Exit, $Log
    if ($Note) { $line += "  ($Note)" }
    Write-Summary $line
    Write-Host $line
}

# Runs one command line through cmd.exe so native stdout and stderr land in one log in
# order, with no PowerShell error records in between. Never throws.
function Invoke-Step([string]$Name, [string]$Command, [string]$Dir = $Repo, [string]$Note = '') {
    $script:Index++
    $slug = ($Name -replace '[^A-Za-z0-9]+', '-').Trim('-').ToLower()
    $logName = '{0:00}-{1}.log' -f $script:Index, $slug
    $log = Join-Path $LogDir $logName
    Set-Content -Path $log -Value "# $Name`r`n# cwd: $Dir`r`n# $Command`r`n# started $(Get-Date -Format s)`r`n" -Encoding ascii
    Write-Host "==> [$($script:Index)] $Name"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & cmd.exe /d /c "cd /d `"$Dir`" && $Command >> `"$log`" 2>&1"
    $code = $LASTEXITCODE
    $sw.Stop()
    Add-Content -Path $log -Value "`r`n# finished $(Get-Date -Format s), exit $code, took $(Format-Duration $sw.Elapsed)" -Encoding ascii
    $status = if ($code -eq 0) { 'PASS' } else { 'FAIL' }
    Add-Result $Name $status $sw.Elapsed "$code" $logName $Note
}

function Skip-Step([string]$Name, [string]$Why) {
    $script:Index++
    Add-Result $Name 'SKIP' ([TimeSpan]::Zero) '-' '-' $Why
}

function Test-Http([string]$Url) {
    try {
        $r = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 5
        return $r.StatusCode -eq 200
    } catch {
        return $false
    }
}

function Test-Command([string]$Name) {
    [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Test-Docker {
    if (-not (Test-Command 'docker')) { return $false }
    & cmd.exe /d /c 'docker info >nul 2>&1'
    return $LASTEXITCODE -eq 0
}

# ---- header ----

Push-Location $Repo
$commit = (& git rev-parse HEAD 2>$null)
$dirty = (& git status --porcelain --untracked-files=no 2>$null)
Pop-Location
Write-Summary "warden overnight run $Stamp"
Write-Summary "commit   $commit$(if ($dirty) { ' (uncommitted changes present)' })"
Write-Summary "started  $(Get-Date -Format s)"
Write-Summary "rustc    $(& rustc -V 2>$null)"
Write-Summary "python   $(& $Python -V 2>&1)"
Write-Summary "asan dll $(if ($AsanDir) { $AsanDir } else { 'NOT FOUND (fuzz targets will not start)' })"
Write-Summary ''
Write-Summary ('{0,-4} {1,-38} {2,9}  {3,-9} {4}' -f 'RES', 'STEP', 'TOOK', 'EXIT', 'LOG')
$RunClock = [Diagnostics.Stopwatch]::StartNew()

# ---- Rust ----

Invoke-Step 'cargo fmt --check' 'cargo fmt --all -- --check'
Invoke-Step 'cargo clippy -D warnings' 'cargo clippy --workspace --all-targets --all-features -- -D warnings'
# -D warnings so a broken intra-doc link fails the step instead of scrolling past.
$env:RUSTDOCFLAGS = '-D warnings'
Invoke-Step 'cargo doc' 'cargo doc --workspace --no-deps'
Remove-Item Env:\RUSTDOCFLAGS
Invoke-Step 'cargo test' 'cargo test --workspace --all-features'

$spiceKey = if ($env:WARDEN_SPICEDB_KEY) { $env:WARDEN_SPICEDB_KEY } else { 'overnight' }
$spiceUp = Test-Http ($SpiceDbUrl.TrimEnd('/') + '/healthz')
# The difftests pass silently without SpiceDB, so they get their own step: run when
# SpiceDB answers, reported as SKIP when it does not.
Invoke-Step 'cargo test --ignored (no difftest)' 'cargo test --workspace --all-features --exclude warden-difftest -- --ignored'
if ($spiceUp) {
    $env:WARDEN_SPICEDB_URL = $SpiceDbUrl
    $env:WARDEN_SPICEDB_KEY = $spiceKey
    $env:WARDEN_DIFFTEST_REQUIRED = '1'
    # One test at a time, as in difftest.yml: two at once more than double SpiceDB's memory.
    Invoke-Step 'difftest vs SpiceDB (--ignored)' 'cargo test -p warden-difftest --test difftest -- --ignored --nocapture --test-threads=1'
    Remove-Item Env:\WARDEN_DIFFTEST_REQUIRED
} else {
    Skip-Step 'difftest vs SpiceDB (--ignored)' "SpiceDB not reachable at $SpiceDbUrl"
}

# ---- Python ----

$py = Join-Path $Repo 'crates\warden-py'
if (Test-Path $Python) {
    Invoke-Step 'maturin develop' 'maturin develop' $py
    Invoke-Step 'pytest' "`"$Python`" -m pytest -q" $py
    Invoke-Step 'mypy --strict' "`"$Python`" -m mypy --strict tests" $py
} else {
    foreach ($s in 'maturin develop', 'pytest', 'mypy --strict') { Skip-Step $s "no virtualenv at $Venv" }
}

# ---- fuzzing ----

$fuzzTargets = 'fuzz_schema_parse', 'fuzz_tuple_parse', 'fuzz_check_vs_reference'
if ((Test-Command 'cargo-fuzz') -and $AsanDir) {
    Invoke-Step 'fuzz build' 'cargo +nightly fuzz build'
    foreach ($t in $fuzzTargets) {
        # New inputs land in fuzz/corpus/<target> (commit them, docs/07 section 5); crashes and
        # hangs in fuzz/artifacts/<target>. -timeout reports a single input over 60 s.
        Invoke-Step "fuzz $t ($FuzzSeconds s)" "cargo +nightly fuzz run $t fuzz/corpus/$t -- -max_total_time=$FuzzSeconds -timeout=60"
    }
} else {
    $why = if (-not (Test-Command 'cargo-fuzz')) { 'cargo-fuzz not installed' } else { 'MSVC ASan DLL not found' }
    Skip-Step 'fuzz build' $why
    foreach ($t in $fuzzTargets) { Skip-Step "fuzz $t ($FuzzSeconds s)" $why }
}

# ---- mutation testing ----

# Most evaluator tests live in other crates (warden/tests, warden-testkit), so every mutant
# runs the whole workspace's tests. Exit 2 = surviving mutants, 3 = timeouts: review them
# in mutants.out and kill or justify them in docs/MUTANTS.md (T3.6).
$mutants = @(
    @{ Name = 'mutants warden-core eval'; File = 'crates/warden-core/src/eval.rs'; Out = 'mutants-eval' },
    @{ Name = 'mutants warden-schema validator'; File = 'crates/warden-schema/src/compile.rs'; Out = 'mutants-schema' }
)
foreach ($m in $mutants) {
    if (Test-Command 'cargo-mutants') {
        $out = Join-Path $LogDir $m.Out
        Invoke-Step $m.Name "cargo mutants --test-workspace=true --jobs $MutantsJobs -f $($m.File) -o `"$out`""
    } else {
        Skip-Step $m.Name 'cargo-mutants not installed'
    }
}

# ---- benchmarks ----

Invoke-Step 'bench data' 'cargo run --quiet -p warden-benches --bin bench-data -- target/bench-data'
Invoke-Step 'bench Rust (criterion)' 'cargo bench -p warden-benches'
if (Test-Path $Python) {
    Invoke-Step 'maturin develop --release' 'maturin develop --release' $py
    Invoke-Step 'bench Python (pytest-benchmark)' "`"$Python`" -m pytest bench/test_scenarios.py -q --benchmark-json=../../target/bench-py.json --benchmark-save-data" $py
} else {
    Skip-Step 'maturin develop --release' "no virtualenv at $Venv"
    Skip-Step 'bench Python (pytest-benchmark)' "no virtualenv at $Venv"
}
# Its own throwaway container on port 8444, never an existing server: run.py empties the
# datastore it is pointed at unless it started it.
if (Test-Docker) {
    Invoke-Step 'bench SpiceDB (docker)' "`"$Python`" benches/spicedb/run.py --port 8444"
} else {
    Skip-Step 'bench SpiceDB (docker)' 'Docker not available'
}
Invoke-Step 'bench results -> benches/RESULTS.md' "`"$Python`" benches/results.py"

# ---- totals ----

$RunClock.Stop()
$pass = @($Results | Where-Object Status -eq 'PASS').Count
$fail = @($Results | Where-Object Status -eq 'FAIL').Count
$skip = @($Results | Where-Object Status -eq 'SKIP').Count
Write-Summary ''
Write-Summary "finished $(Get-Date -Format s), total $(Format-Duration $RunClock.Elapsed)"
Write-Summary "$pass passed, $fail failed, $skip skipped"
if ($fail) {
    Write-Summary 'failed steps:'
    $Results | Where-Object Status -eq 'FAIL' | ForEach-Object { Write-Summary "  $($_.Name) -> $($_.Log)" }
}
Write-Host ''
Write-Host "Summary: $Summary"
exit $(if ($fail) { 1 } else { 0 })
