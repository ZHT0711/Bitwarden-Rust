function Resolve-FixturePython {
    param([string]$PythonExe = 'python')

    $hint = 'A working Python 3.10+ interpreter is required. Pass -PythonExe with its full path; a Windows Store execution alias alone is not enough.'
    try {
        # Application lookup can yield every matching executable on PATH, including
        # the Microsoft Store alias. Preserve PATH precedence but return one path.
        $command = Get-Command $PythonExe -CommandType Application -TotalCount 1 -ErrorAction Stop
        $probe = & $command.Source -c 'import sys; print("bytewarden-fixture-python3" if sys.version_info >= (3, 10) else "unsupported")' | Out-String
        if ($LASTEXITCODE -eq 0 -and $probe.Trim() -eq 'bytewarden-fixture-python3') {
            return $command.Source
        }
    } catch {
        throw [InvalidOperationException]::new($hint, $_.Exception)
    }
    throw $hint
}
