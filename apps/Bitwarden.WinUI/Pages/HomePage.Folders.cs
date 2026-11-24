using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

public sealed partial class HomePage
{
    internal event EventHandler? ManageFoldersRequested;
    private FolderHierarchy _folderHierarchy = FolderHierarchy.Build([], []);
    private FolderHierarchy? _navigationHierarchy;
    private string? _navigationFolderId;
    private Dictionary<string, FolderChoice> _folderChoices = new(StringComparer.Ordinal);

    private void UpdateFolderNavigation(string folderId)
    {
        var current = _folderHierarchy.Find(folderId);
        if (ReferenceEquals(_navigationHierarchy, _folderHierarchy) && _navigationFolderId == current?.Id) return;
        _navigationHierarchy = _folderHierarchy;
        _navigationFolderId = current?.Id;
        FolderNavigationPanel.Visibility = current is null ? Visibility.Collapsed : Visibility.Visible;
        var navigation = current is null ? (Visible: Array.Empty<FolderCrumb>(), Earlier: Array.Empty<FolderRow>())
            : _folderHierarchy.Navigation(current.Id);
        FolderCrumbs.ItemsSource = navigation.Visible;
        FolderAncestorsFlyout.Hide();
        FolderAncestorList.ItemsSource = navigation.Earlier;
        FolderAncestorsButton.Content = $"更早的上级（{navigation.Earlier.Length}）";
        FolderAncestorsButton.Visibility = navigation.Earlier.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        ChildFolderList.ItemsSource = current?.Children.Select(node => node.Row(canEdit: false)).ToArray();
        ChildFolderSection.Visibility = current is { Children.Length: > 0 } ? Visibility.Visible : Visibility.Collapsed;
    }

    private void FolderCrumbs_ItemClicked(BreadcrumbBar sender, BreadcrumbBarItemClickedEventArgs args)
    {
        if (!FolderFilter.IsEnabled || !_loaded || _editorOpen || args.Item is not FolderCrumb crumb) return;
        if (crumb.Id is null) ManageFoldersRequested?.Invoke(this, EventArgs.Empty);
        else SelectNestedFolder(crumb.Id);
    }

    private void ChildFolderList_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is FolderRow folder) SelectNestedFolder(folder.Id);
    }

    private void FolderAncestor_Click(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: FolderRow folder }) SelectNestedFolder(folder.Id);
        FolderAncestorsFlyout.Hide();
    }

    private void SelectNestedFolder(string id)
    {
        if (!FolderFilter.IsEnabled || !_loaded || _editorOpen) return;
        if (_folderChoices.TryGetValue(id, out var choice)) FolderFilter.SelectedItem = choice;
    }
}
