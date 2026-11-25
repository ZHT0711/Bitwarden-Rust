using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Bytewarden.WinUI.Controls;

/// <summary>Label/container identity is assigned only for realized, data-bound folder rows.</summary>
internal static class FolderPicker
{
    internal static void SetContainerIdentity(FrameworkElement label)
    {
        if (label.DataContext is not FolderChoice choice) return;
        for (DependencyObject? parent = VisualTreeHelper.GetParent(label); parent is not null;
            parent = VisualTreeHelper.GetParent(parent))
        {
            if (parent is not ComboBoxItem) continue;
            AutomationProperties.SetAutomationId(parent, choice.AutomationId);
            AutomationProperties.SetName(parent, choice.DisplayName);
            break;
        }
    }
}
