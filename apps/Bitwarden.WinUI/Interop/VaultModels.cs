namespace Bitwarden_WinUI.Interop;

internal sealed record VaultOverview(VaultItem[] Items, VaultFolder[] Folders, DateTimeOffset LastSync, int Failures);
internal sealed record VaultFolder(string Id, string Name);
public sealed record VaultItem(string Id, string Name, string Subtitle, int ItemType, bool Favorite,
    string? FolderId, string? OrganizationId, bool Deleted, bool Archived, bool Reprompt, bool Restricted)
{
    public string DisplayName => string.IsNullOrEmpty(Name) ? "（未命名条目）" : Name;
    public override string ToString() => DisplayName;
    public string TypeLabel => VaultTypes.Label(ItemType);
    public string StatusLabel => Restricted ? "访问受限" : Deleted ? "已删除" : Archived ? "已归档" : Favorite ? "收藏" : TypeLabel;
}
internal sealed record VaultDetail(string Name, int ItemType, DisplayField[] Fields, DateTimeOffset RevisionDate,
    bool Restricted, bool PasswordHidden, int Attachments);
internal sealed record DisplayField(string Label, string Value, bool Sensitive);

internal sealed class VaultException(string code) : Exception(code)
{
    public string Code { get; } = code;
    public string UserMessage => Code switch
    {
        "notAuthenticated" => "当前没有活跃的账户会话，请先登录。",
        "network" => "无法同步。请检查网络连接和登录状态后重试；上次成功加载的条目仍然保留。",
        "invalidResponse" => "服务器返回的保险库数据不完整或格式不受支持。",
        "notFound" => "未找到条目或尚未完成首次同步，请重新同步。",
        "decryptionFailed" => "无法解密保险库数据，请重新登录后重试。",
        "passwordRequired" => "此条目需要重新验证主密码。",
        "wrongPassword" => "主密码验证失败，请重试。",
        "timedOut" => "服务器响应超时，请检查网络连接后重试。",
        _ => "Rust 核心未能完成操作，请重试或修复应用安装。",
    };
}

internal static class VaultTypes
{
    internal static string Label(int type) => type switch
    {
        1 => "登录", 2 => "安全笔记", 3 => "银行卡", 4 => "身份信息",
        5 => "SSH 密钥", 6 => "银行账户", 7 => "驾驶执照", 8 => "护照", _ => "其他",
    };
}
