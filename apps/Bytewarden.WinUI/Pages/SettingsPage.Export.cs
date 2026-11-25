using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;

namespace Bytewarden.WinUI.Pages;

public sealed partial class SettingsPage
{
    internal Func<Task>? ExportVault { get; set; }
    internal Func<Task>? ImportVault { get; set; }
    private string? _exportAccount;

    internal void ShowVaultExport(VaultAccount? account, bool busy)
    {
        if (_exportAccount != account?.Id || account?.State != VaultSessionState.Unlocked)
            ExportVaultStatus.Text = string.Empty;
        _exportAccount = account?.Id;
        ExportVaultButton.IsEnabled = ImportVaultButton.IsEnabled = !busy && account is { State: VaultSessionState.Unlocked, NeedsSignIn: false };
    }

    internal void ReportVaultExport(string message) => ExportVaultStatus.Text = message;

    private async void ExportVault_Click(object sender, RoutedEventArgs e)
    {
        if (ExportVault is not null) await ExportVault();
    }

    private async void ImportVault_Click(object sender, RoutedEventArgs e)
    {
        if (ImportVault is not null) await ImportVault();
    }
}
