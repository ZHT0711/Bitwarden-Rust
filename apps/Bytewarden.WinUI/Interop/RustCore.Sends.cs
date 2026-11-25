using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Bytewarden.WinUI.Interop;

internal sealed record SendOverview(SendSummary[] Items, int Failures);

public sealed record SendSummary(string Id, string Name, int Kind, bool Disabled,
    DateTimeOffset DeletionDate, DateTimeOffset? ExpirationDate, string Revision)
{
    public string KindLabel => Kind switch { 0 => "文本", 1 => "文件", 2 => "保险库条目", _ => "未知类型" };
    public string StatusLabel => Disabled ? "已停用" : DeletionDate <= DateTimeOffset.UtcNow ? "待删除" :
        ExpirationDate <= DateTimeOffset.UtcNow ? "已过期" : "有效";
    public string AccessibilityName => $"{Name} · {KindLabel} · {StatusLabel}";
    public override string ToString() => Name;
}

internal sealed record SendDetail(string Id, string Name, int Kind, bool Disabled, string DeletionDate,
    string? ExpirationDate, string Revision, string? Notes, string? Text, string? FileName, bool Hidden,
    bool HideEmail, uint? MaxAccessCount, uint AccessCount, int AuthType, bool HasPassword, string[] Emails)
{
    public override string ToString() => Name;
}

internal sealed record SendDraft(string Name, string? Text, string? Notes, bool Hidden, bool Disabled,
    bool HideEmail, uint? MaxAccessCount, string DeletionDate, string? ExpirationDate,
    string? Password, string[]? Emails)
{
    public override string ToString() => Name;
}

internal static partial class RustCore
{
    private sealed record SendRequest(string AccountId, SendCommand Command);
    private sealed record SendCommand(string Operation,
        [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] string? Id = null,
        [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] string? Revision = null,
        [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] SendDraft? Data = null,
        [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] string? Source = null)
    {
        public override string ToString() => Operation;
    }

    [DllImport(LibraryName, EntryPoint = "bw_send_command", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr SendCommandNative(IntPtr command, ulong requestId);

    internal static SendOverview ListSends(string account, CancellationToken token) =>
        RunSend<SendOverview>(account, new("list"), token);

    internal static SendDetail GetSend(string account, string id, CancellationToken token) =>
        RunSend<SendDetail>(account, new("detail", id), token);

    internal static string GetSendLink(string account, string id, CancellationToken token) =>
        RunSend<string>(account, new("link", id), token);

    internal static void SaveSend(string account, SendDetail? item, SendDraft draft, CancellationToken token) =>
        RunSend<bool>(account, item is null ? new("create", Data: draft) : new("edit", item.Id, item.Revision, draft), token, write: true);

    internal static void CreateFileSend(string account, SendDraft draft, string source, CancellationToken token) =>
        RunSend<bool>(account, new("createFile", Data: draft, Source: source), token, write: true);

    internal static void DeleteSend(string account, SendSummary item, CancellationToken token) =>
        RunSend<bool>(account, new("delete", item.Id, item.Revision), token, write: true);

    internal static void RemoveSendAuth(string account, SendDetail item, CancellationToken token) =>
        RunSend<bool>(account, new("removeAuth", item.Id, item.Revision), token, write: true);

    private static T RunSend<T>(string account, SendCommand command, CancellationToken token, bool write = false)
    {
        using var buffer = new Utf8Buffer(JsonSerializer.Serialize(new SendRequest(account, command), JsonInfo<SendRequest>()));
        return RunVaultRequest<T>(id => SendCommandNative(buffer.Pointer, id), token, acknowledgedWrite: write);
    }
}
