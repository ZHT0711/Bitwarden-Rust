namespace Bitwarden_WinUI.Interop;

internal sealed record VaultOverview(VaultItem[] Items, VaultFolder[] Folders, DateTimeOffset LastSync, int Failures);
internal sealed record VaultFolder(string Id, string Name);
public sealed record VaultItem(string Id, string Name, string Subtitle, int ItemType, bool Favorite,
    string? FolderId, string? OrganizationId, bool Deleted, bool Archived, bool Reprompt, bool Restricted, string RevisionDate = "",
    string? WebsiteHost = null)
{
    public string DisplayName => string.IsNullOrEmpty(Name) ? "（未命名条目）" : Name;
    public override string ToString() => DisplayName;
    public string TypeLabel => VaultTypes.Label(ItemType);
    public string StatusLabel => Restricted ? "访问受限" : Deleted ? "已删除" : Archived ? "已归档" : Favorite ? "收藏" : TypeLabel;
}
internal sealed record VaultDetail(string Name, int ItemType, DisplayField[] Fields, DateTimeOffset RevisionDate,
    bool Restricted, bool PasswordHidden, int Attachments, VaultTotp? Totp = null, ulong? TotpRequestId = null,
    VaultPasskey[]? Passkeys = null);
internal sealed record DisplayField(string Label, string Value, bool Sensitive);
internal sealed record VaultTotp(string Status, TotpCode? Value);
internal sealed record TotpCode(string Code, uint Period, long ValidFrom, long ValidUntil);
internal sealed record VaultPasskey(string RelyingParty, string? UserName, DateTimeOffset CreatedAt)
{
    internal string DisplayText => string.IsNullOrWhiteSpace(UserName) ? RelyingParty : $"{RelyingParty} · {UserName}";
}

internal sealed class VaultException(string code) : Exception(code)
{
    public string Code { get; } = code;
    public string UserMessage => Code switch
    {
        "notAuthenticated" => "当前没有活跃的账户会话，请先登录。",
        "locked" => "保险库已锁定，请先解锁。",
        "unlockUnavailable" => "本次会话需要重新登录。请取消解锁，选择「退出登录」，然后重新连接账户。",
        "network" => "无法同步。请检查网络连接和登录状态后重试。",
        "invalidResponse" => "服务器返回的保险库数据不完整或格式不受支持。",
        "notFound" => "未找到条目或尚未完成首次同步，请重新同步。",
        "decryptionFailed" => "无法解密保险库数据，请重新登录后重试。",
        "passwordRequired" => "此条目需要重新验证主密码。",
        "wrongPassword" => "主密码验证失败，请重试。",
        "timedOut" => "服务器响应超时，请检查网络连接后重试。",
        "validation" => "条目信息无效，请检查名称、日期、URI 匹配模式和自定义字段。",
        "permissionDenied" => "当前账户没有执行此操作所需的权限。",
        "premiumRequired" => "归档需要个人或组织提供的高级会员资格；取消归档仍然可用。",
        "attachmentUnavailable" => "此条目有未能解密的附件。为避免编辑时丢失附件引用，请先同步并处理附件问题。",
        "conflict" => "此条目已经发生变化。草稿已保留，请先同步并核对最新版本，避免覆盖其他修改。",
        "writeUncertain" => "服务器可能已经保存了此操作，但未能确认结果。草稿已保留，请先同步核对，勿直接重复创建或提交。",
        "notSynced" => "请先成功同步保险库，再继续读取或写入条目。",
        "totpViewExpired" => "请重新打开条目以查看验证码。",
        "unsupported" => "此条目或操作暂不受支持。",
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
