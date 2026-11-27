using Bytewarden.WinUI.Interop;

internal static class AuthStatusChecks
{
    internal static void Run(Action<bool, string> check)
    {
        check((uint)AuthStatus.Failed == 0 && (uint)AuthStatus.RateLimited == 10,
            "existing authentication ABI status values remain stable");
        check((uint)AuthStatus.NetworkError == 11 && (uint)AuthStatus.ServerError == 12 &&
            (uint)AuthStatus.AuthenticationRejected == 13,
            "appended authentication ABI statuses have the expected values");
        foreach (var status in new[] { AuthStatus.NetworkError, AuthStatus.ServerError,
            AuthStatus.AuthenticationRejected, AuthStatus.Failed })
        {
            var message = AuthStatusMessages.LoginStatusMessage(status);
            check(!message.Contains("密码错误", StringComparison.Ordinal) &&
                !message.Contains("主密码错误", StringComparison.Ordinal) &&
                !message.Contains("wrong password", StringComparison.OrdinalIgnoreCase),
                $"{status} message does not claim a wrong password");
        }
        check(AuthStatusMessages.LoginStatusMessage(AuthStatus.NetworkError).Contains("网络", StringComparison.Ordinal),
            "network status has a fixed localized label");
        check(AuthStatusMessages.LoginStatusMessage(AuthStatus.ServerError).Contains("服务器", StringComparison.Ordinal),
            "server status has a fixed localized label");
        check(AuthStatusMessages.LoginStatusMessage(AuthStatus.AuthenticationRejected).Contains("未接受", StringComparison.Ordinal),
            "authentication rejection label remains neutral about its cause");
        check(AuthStatusMessages.LoginStatusMessage(AuthStatus.Failed).Contains("原因无法确定", StringComparison.Ordinal),
            "unknown authentication failure remains explicitly unknown");
    }
}
