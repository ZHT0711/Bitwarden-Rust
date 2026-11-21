using System.Security.Cryptography;
using System.Text;

namespace Bitwarden_WinUI.Interop;

internal static class AccountSettingsScope
{
    internal static string? Current { get; private set; }
    internal static event Action<string>? SignedOut;

    internal static string Create(uint serverMode, string address, string email)
    {
        var server = serverMode == 2
            ? new Uri(address).GetComponents(UriComponents.SchemeAndServer | UriComponents.Path, UriFormat.UriEscaped).TrimEnd('/')
            : serverMode == 1 ? "https://vault.bitwarden.eu" : "https://vault.bitwarden.com";
        var input = Encoding.UTF8.GetBytes($"{serverMode}\n{server}\n{email.Trim().ToLowerInvariant()}");
        return Convert.ToHexString(SHA256.HashData(input));
    }

    internal static void Begin(uint serverMode, string address, string email) => Current = Create(serverMode, address, email);
    internal static void Use(string? scope) => Current = scope;
    internal static void NotifySignedOut(string scope) => SignedOut?.Invoke(scope);

    internal static void End()
    {
        var previous = Current;
        Current = null;
        if (previous is not null) SignedOut?.Invoke(previous);
    }
}
