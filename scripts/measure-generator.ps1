param(
    [string] $LibraryPath = (Join-Path $PSScriptRoot "../target/debug/bytewarden_core.dll"),
    [ValidateRange(10, 100000)]
    [int] $Iterations = 300
)

$ErrorActionPreference = "Stop"
$LibraryPath = (Resolve-Path -LiteralPath $LibraryPath).Path

Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;

public static class GeneratorMeasurement
{
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate IntPtr Password(uint length, uint options, uint minNumbers, uint minSpecial);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate IntPtr Passphrase(uint words, IntPtr separator, byte capitalize, byte number);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void Free(IntPtr value);

    public static object Measure(string libraryPath, int iterations)
    {
        var library = NativeLibrary.Load(libraryPath);
        var separator = Marshal.StringToCoTaskMemUTF8("-");
        try
        {
            var password = Marshal.GetDelegateForFunctionPointer<Password>(NativeLibrary.GetExport(library, "bw_generate_password"));
            var passphrase = Marshal.GetDelegateForFunctionPointer<Passphrase>(NativeLibrary.GetExport(library, "bw_generate_passphrase"));
            var free = Marshal.GetDelegateForFunctionPointer<Free>(NativeLibrary.GetExport(library, "bw_string_free"));
            Action<IntPtr> consume = value => {
                if (value == IntPtr.Zero) throw new InvalidOperationException("Generator returned no result.");
                free(value);
            };
            var coldStart = Stopwatch.StartNew();
            consume(password(24, 15, 2, 2));
            coldStart.Stop();
            // Warm up the runtime and SDK word list without printing generated secrets.
            for (var i = 0; i < 5; i++) {
                consume(password(24, 15, 2, 2));
                consume(passphrase(6, separator, 0, 0));
            }
            return new {
                Library = libraryPath,
                IterationsPerMode = iterations,
                ColdPasswordMilliseconds = coldStart.Elapsed.TotalMilliseconds,
                Password = Sample(iterations, () => consume(password(24, 15, 2, 2))),
                Passphrase = Sample(iterations, () => consume(passphrase(6, separator, 0, 0)))
            };
        }
        finally
        {
            Marshal.FreeCoTaskMem(separator);
            // Keep the library loaded until process exit: SDK runtime threads can outlive a call.
        }
    }

    private static object Sample(int iterations, Action generate)
    {
        var times = new double[iterations];
        var total = Stopwatch.StartNew();
        for (var i = 0; i < iterations; i++) {
            var start = Stopwatch.GetTimestamp();
            generate();
            times[i] = (Stopwatch.GetTimestamp() - start) * 1000.0 / Stopwatch.Frequency;
        }
        total.Stop();
        Array.Sort(times);
        return new {
            TotalMilliseconds = total.Elapsed.TotalMilliseconds,
            MedianMilliseconds = times[iterations / 2],
            P95Milliseconds = times[(int)Math.Ceiling(iterations * 0.95) - 1]
        };
    }
}
'@

[GeneratorMeasurement]::Measure($LibraryPath, $Iterations) | ConvertTo-Json -Depth 4
