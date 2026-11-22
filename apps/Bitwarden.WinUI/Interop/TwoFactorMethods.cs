namespace Bitwarden_WinUI.Interop;

internal static class TwoFactorMethods
{
    internal const uint Authenticator = 0;
    internal const uint Email = 1;
    internal const uint YubiKey = 3;
    internal const uint WebAuthn = 7;
    private const uint Supported = (1u << (int)Authenticator) | (1u << (int)Email) | (1u << (int)YubiKey);

    internal static bool IsAdvertised(uint advertised, uint provider) => (advertised & (1u << (int)provider)) != 0;

    // Some self-hosted deployments return a challenge with no supported provider in its metadata.
    // The UI may offer an explicit TOTP attempt; the host still has to accept that exact token.
    internal static bool OffersManualAuthenticator(uint advertised, uint serverMode) =>
        serverMode == 2 && (advertised & Supported) == 0;

    internal static string UnsupportedReason(uint advertised) => advertised switch
    {
        1u << (int)WebAuthn => "服务器此次只列出了通行密钥，没有列出身份验证器。",
        0 => "服务器没有列出此应用可识别的二步验证方式。",
        _ => "服务器没有列出此应用支持的二步验证方式。",
    };
}
