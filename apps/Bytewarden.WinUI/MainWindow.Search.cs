using Bytewarden.WinUI.Interop;
using Bytewarden.WinUI.Pages;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Bytewarden.WinUI;

public sealed partial class MainWindow
{
    private TextBox? _searchInput;
    private SearchDispatch? _searchDispatch;

    private void SearchBox_Loaded(object sender, RoutedEventArgs args) => BindSearchInput();

    private void SearchBox_Unloaded(object sender, RoutedEventArgs args)
    {
        UnbindSearchInput();
        _searchDispatch?.Cancel();
    }

    private void BindSearchInput()
    {
        if (_closed || !SearchBox.IsLoaded) return;
        var input = FindSearchInput(SearchBox);
        if (ReferenceEquals(input, _searchInput)) return;
        UnbindSearchInput();
        _searchInput = input;
        if (input is null) return; // Keep the public AutoSuggestBox event as the template fallback.
        input.TextChanged += SearchInput_TextChanged;
        input.Unloaded += SearchInput_Unloaded;
    }

    private void UnbindSearchInput()
    {
        if (_searchInput is not { } input) return;
        input.TextChanged -= SearchInput_TextChanged;
        input.Unloaded -= SearchInput_Unloaded;
        _searchInput = null;
    }

    private void SearchInput_Unloaded(object sender, RoutedEventArgs args)
    {
        if (!ReferenceEquals(sender, _searchInput)) return;
        UnbindSearchInput();
        _searchDispatch?.Cancel();
        // Re-templating can replace the inner edit without unloading the AutoSuggestBox itself.
        if (!_closed) DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, BindSearchInput);
    }

    private void SearchInput_TextChanged(object sender, TextChangedEventArgs args) =>
        RequestSearch(((TextBox)sender).Text);

    private void RequestSearch(string text)
    {
        if (_closed || NavFrame.Content is not (HomePage or FolderManagementPage or SendsPage)) return;
        _searchDispatch ??= new SearchDispatch(
            action => DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () => action()),
            (target, query) =>
            {
                if (_closed || !ReferenceEquals(target, NavFrame.Content)) return;
                if (target is HomePage page) page.SetSearch(query);
                else if (target is FolderManagementPage folders) folders.SetSearch(query);
                else if (target is SendsPage sends) sends.SetSearch(query);
            });
        _searchDispatch.Request(NavFrame.Content, text);
    }

    private static TextBox? FindSearchInput(DependencyObject parent)
    {
        for (var index = 0; index < VisualTreeHelper.GetChildrenCount(parent); index++)
        {
            var child = VisualTreeHelper.GetChild(parent, index);
            if (child is TextBox input) return input;
            if (FindSearchInput(child) is { } descendant) return descendant;
        }
        return null;
    }
}
