using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Threading;

namespace NotchMusic;

public partial class MainWindow
{
    private readonly ClipboardHistoryBuffer _clipboardHistory = new();
    private readonly DispatcherTimer _clipboardSearchTimer = new() { Interval = TimeSpan.FromMilliseconds(140) };
    private bool _clipboardListening, _clipboardReadBusy, _clipboardReadPending, _clipboardCopyBusy;
    private bool _clipboardResultsDirty = true;
    private uint _clipboardLastSequence;
    private int _clipboardEpoch;
    private int _expandedPage, _pageAnimationVersion, _queuedPageDirection;
    private bool _pageAnimationBusy;

    private void InitializeClipboardPages()
    {
        _expandedPage = Math.Clamp(_preferences.ExpandedPage, 0, PageCount - 1);
        _clipboardSearchTimer.Tick += (_, _) =>
        { _clipboardSearchTimer.Stop(); RefreshClipboardResults(); };
        CompletePageTransition();
        RefreshClipboardResults();
        UpdateClipboardHeader();
    }

    private void StartClipboardListener()
    {
        // Jangan mengimpor isi clipboard lama saat startup: sesi baru selalu kosong.
        _clipboardLastSequence = ClipboardNative.Sequence;
        _clipboardListening = ClipboardNative.AddClipboardFormatListener(_windowHandle);
        if (!_clipboardListening) ClipboardStatus.Text = "Pencatatan belum aktif. Tutup dan buka aplikasi lagi.";
    }

    private void QueueClipboardRead()
    {
        if (_closed || !_preferences.ClipboardRecordingEnabled) return;
        _clipboardReadPending = true;
        if (_clipboardReadBusy) return;
        _clipboardReadBusy = true;
        _ = ReadClipboardUpdatesAsync();
    }

    private async Task ReadClipboardUpdatesAsync()
    {
        try
        {
            do
            {
                _clipboardReadPending = false;
                if (_closed || !_preferences.ClipboardRecordingEnabled) return;
                if (ClipboardNative.Sequence == _clipboardLastSequence) continue;
                var epoch = _clipboardEpoch;
                ClipboardRead result = default;
                // Retry singkat hanya bila clipboard dipakai aplikasi lain, tanpa polling idle.
                for (var attempt = 0; attempt < 3; attempt++)
                {
                    if (_closed || epoch != _clipboardEpoch) break;
                    result = await Task.Run(() => ClipboardNative.Read(_windowHandle));
                    if (result.Kind != ClipboardReadKind.Busy) break;
                    if (attempt < 2) await Task.Delay(45);
                }
                if (_closed) return;
                if (epoch != _clipboardEpoch || !_preferences.ClipboardRecordingEnabled) continue;
                if (result.Kind == ClipboardReadKind.Busy) continue;
                _clipboardLastSequence = result.Sequence;
                if (result.Kind == ClipboardReadKind.TooLarge)
                { ClipboardStatus.Text = "Teks terlalu besar; maksimal 16.384 karakter per salinan."; continue; }
                if (result.Kind != ClipboardReadKind.Text || result.Text is null) continue;
                if (_clipboardHistory.Add(result.Text, DateTimeOffset.Now))
                {
                    _clipboardResultsDirty = true;
                    ClipboardStatus.Text = "Tersimpan untuk sesi ini · hilang saat aplikasi ditutup";
                    UpdateClipboardHeader();
                    if (_expanded && _expandedPage == 1 && !_minimizedToTray) RefreshClipboardResults();
                }
            } while (_clipboardReadPending);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine(ex.GetType().Name);
            if (!_closed) ClipboardStatus.Text = "Clipboard belum bisa dibaca. Coba salin teks lagi.";
        }
        finally { _clipboardReadBusy = false; }
    }

    private void UpdateClipboardHeader()
    {
        ClipboardCount.Text = $"{_clipboardHistory.Items.Count} / {ClipboardHistoryBuffer.MaxItems}";
        ClipboardPauseButton.Content = _preferences.ClipboardRecordingEnabled ? "Jeda" : "Lanjut";
        ClipboardPauseButton.ToolTip = _preferences.ClipboardRecordingEnabled ? "Jeda pencatatan clipboard" : "Lanjutkan pencatatan clipboard";
        ClipboardClearButton.IsEnabled = _clipboardHistory.Items.Count > 0;
    }

    private void ClipboardSearch_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (_closed) return;
        _clipboardResultsDirty = true;
        _clipboardSearchTimer.Stop();
        if (_expanded && _expandedPage == 1) _clipboardSearchTimer.Start();
    }

    private void RefreshClipboardResults()
    {
        _clipboardSearchTimer.Stop();
        if (_closed) return;
        var matches = _clipboardHistory.Find(ClipboardSearchBox.Text);
        ClipboardList.ItemsSource = matches;
        ClipboardEmpty.Visibility = matches.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        ClipboardEmpty.Text = _clipboardHistory.Items.Count == 0
            ? "Salin teks dengan Ctrl+C untuk memulai."
            : "Tidak ada salinan yang cocok dengan pencarian.";
        _clipboardResultsDirty = false;
    }

    private void ClipboardPause_Click(object sender, RoutedEventArgs e)
    {
        _preferences.ClipboardRecordingEnabled = !_preferences.ClipboardRecordingEnabled;
        _clipboardEpoch++; _clipboardReadPending = false;
        _clipboardLastSequence = ClipboardNative.Sequence;
        UpdateClipboardHeader(); SavePreferences();
        ClipboardStatus.Text = _preferences.ClipboardRecordingEnabled
            ? "Pencatatan dilanjutkan untuk salinan berikutnya."
            : "Pencatatan dijeda. Riwayat sesi tetap tersedia.";
    }

    private void ClipboardClear_Click(object sender, RoutedEventArgs e)
    {
        // Hasil baca yang masih menunggu tidak boleh mengisi kembali daftar setelah Clear.
        _clipboardEpoch++; _clipboardReadPending = false;
        _clipboardLastSequence = ClipboardNative.Sequence;
        _clipboardHistory.Clear();
        ClipboardSearchBox.Clear();
        RefreshClipboardResults(); UpdateClipboardHeader();
        ClipboardStatus.Text = "Riwayat notch sudah dikosongkan.";
    }

    private void ClipboardDelete_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: ClipboardItem item } && _clipboardHistory.Remove(item.Id))
        {
            _clipboardEpoch++; _clipboardReadPending = false;
            _clipboardLastSequence = ClipboardNative.Sequence;
            RefreshClipboardResults(); UpdateClipboardHeader(); ClipboardStatus.Text = "Salinan dihapus dari riwayat sesi.";
        }
    }

    private async void ClipboardCopy_Click(object sender, RoutedEventArgs e)
    {
        if (_clipboardCopyBusy || _closed || sender is not Button { Tag: ClipboardItem item }) return;
        _clipboardCopyBusy = true;
        try
        {
            var copied = false;
            for (var attempt = 0; attempt < 3 && !_closed; attempt++)
            {
                copied = await Task.Run(() => ClipboardNative.Write(_windowHandle, item.Text));
                if (copied) break;
                if (attempt < 2) await Task.Delay(45);
            }
            if (!_closed) ClipboardStatus.Text = copied
                ? "Disalin. Tempel ke aplikasi tujuan dengan Ctrl+V."
                : "Clipboard sedang dipakai aplikasi lain. Coba Salin lagi.";
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine(ex.GetType().Name);
            if (!_closed) ClipboardStatus.Text = "Teks belum bisa disalin. Coba lagi.";
        }
        finally { _clipboardCopyBusy = false; }
    }

    private void PreviousPage_Click(object sender, RoutedEventArgs e) => NavigatePage(-1);
    private void NextPage_Click(object sender, RoutedEventArgs e) => NavigatePage(1);

    // Urutan halaman notch terbuka: musik, clipboard, game.
    private const int PageCount = 3;
    private Grid PageAt(int index) => index switch { 0 => MusicPage, 1 => ClipboardPage, _ => GamePage };
    private TranslateTransform ShiftAt(int index) => index switch { 0 => MusicPageShift, 1 => ClipboardPageShift, _ => GamePageShift };

    private void NavigatePage(int direction)
    {
        if (_closed || _minimizedToTray || _repositioning || _seekDragging) return;
        if (_pageAnimationBusy) { _queuedPageDirection = Math.Sign(direction); return; }
        var previous = _expandedPage;
        _expandedPage = NotchPages.Next(previous, direction, PageCount);
        _preferences.ExpandedPage = _expandedPage; SavePreferences();
        StopLyricScroll();
        if (_expandedPage == 1 && _clipboardResultsDirty) RefreshClipboardResults();
        if (_expandedPage == 0) { RenderLyrics(); RenderProgress(); }
        if (_expandedPage == GamePageIndex) RefreshGames();
        UpdateProgressTimer(); UpdateLyricTimer();
        if (ClipboardSearchBox.IsKeyboardFocusWithin) Keyboard.ClearFocus();
        if (!SystemParameters.ClientAreaAnimation || !_expanded)
        { CompletePageTransition(); return; }

        _pageAnimationBusy = true;
        var version = ++_pageAnimationVersion;
        var outgoing = PageAt(previous);
        var incoming = PageAt(_expandedPage);
        var oldShift = ShiftAt(previous);
        var newShift = ShiftAt(_expandedPage);
        var distance = PageViewport.Width * Math.Sign(direction);
        outgoing.Visibility = incoming.Visibility = Visibility.Visible;
        outgoing.IsHitTestVisible = incoming.IsHitTestVisible = false;
        outgoing.Opacity = 1; incoming.Opacity = 0;
        oldShift.X = 0; newShift.X = distance;
        var duration = TimeSpan.FromMilliseconds(LightAnimations ? 180 : 240);
        DoubleAnimation Animation(double from, double to)
        {
            var animation = new DoubleAnimation(from, to, duration)
            { EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }, FillBehavior = FillBehavior.HoldEnd };
            Timeline.SetDesiredFrameRate(animation, AnimationFrames);
            return animation;
        }
        var enter = Animation(distance, 0);
        enter.Completed += (_, _) =>
        {
            if (_closed || version != _pageAnimationVersion) return;
            var queued = _queuedPageDirection;
            CompletePageTransition();
            if (queued != 0) NavigatePage(queued);
        };
        oldShift.BeginAnimation(TranslateTransform.XProperty, Animation(0, -distance), HandoffBehavior.SnapshotAndReplace);
        newShift.BeginAnimation(TranslateTransform.XProperty, enter, HandoffBehavior.SnapshotAndReplace);
        outgoing.BeginAnimation(OpacityProperty, Animation(1, 0), HandoffBehavior.SnapshotAndReplace);
        incoming.BeginAnimation(OpacityProperty, Animation(0, 1), HandoffBehavior.SnapshotAndReplace);
    }

    private void CompletePageTransition()
    {
        ++_pageAnimationVersion; _pageAnimationBusy = false; _queuedPageDirection = 0;
        for (var i = 0; i < PageCount; i++)
        {
            var page = PageAt(i); var shift = ShiftAt(i);
            shift.BeginAnimation(TranslateTransform.XProperty, null); shift.X = 0;
            page.BeginAnimation(OpacityProperty, null); page.Opacity = 1;
            page.Visibility = _expandedPage == i ? Visibility.Visible : Visibility.Collapsed;
            page.IsHitTestVisible = _expandedPage == i;
        }
    }

    private void StopClipboardFeature()
    {
        if (_clipboardListening) ClipboardNative.RemoveClipboardFormatListener(_windowHandle);
        _clipboardListening = false; _clipboardEpoch++; _clipboardReadPending = false;
        _clipboardSearchTimer.Stop(); CompletePageTransition();
        ClipboardList.ItemsSource = null; ClipboardSearchBox.Clear();
        _clipboardHistory.Clear(); // Tidak ada file, snapshot disk, atau pemulihan clipboard antar sesi.
    }
}

internal static class NotchPages
{
    internal static int Next(int current, int direction, int count)
    {
        if (count < 1) throw new ArgumentOutOfRangeException(nameof(count));
        return ((current % count + Math.Sign(direction)) % count + count) % count;
    }
}

internal sealed record ClipboardItem(Guid Id, string Text, DateTimeOffset CopiedAt)
{
    public string Preview { get; } = MakePreview(Text, 180);
    public string Tooltip { get; } = MakePreview(Text, 500);
    public string TimeLabel => CopiedAt.ToLocalTime().ToString("HH:mm", CultureInfo.CurrentCulture);

    private static string MakePreview(string text, int maximum)
    {
        var length = Math.Min(text.Length, maximum);
        if (length > 0 && char.IsHighSurrogate(text[length - 1])) length--;
        var builder = new StringBuilder(length + 1);
        for (var i = 0; i < length; i++) builder.Append(char.IsWhiteSpace(text[i]) ? ' ' : text[i]);
        if (length < text.Length) builder.Append('…');
        return builder.ToString();
    }
}

// Teks hanya hidup selama sesi aplikasi. Batas juga berlaku untuk seluruh isi, bukan hanya jumlah baris UI.
internal sealed class ClipboardHistoryBuffer
{
    internal const int MaxItems = 64;
    internal const int MaxTextCharacters = 16384;
    internal const int MaxTotalCharacters = 262144;
    private readonly List<ClipboardItem> _items = [];
    private int _characters;
    internal IReadOnlyList<ClipboardItem> Items => _items;

    internal bool Add(string text, DateTimeOffset copiedAt)
    {
        if (string.IsNullOrWhiteSpace(text) || text.Length > MaxTextCharacters || text.Contains('\0')) return false;
        var duplicate = _items.FindIndex(item => string.Equals(item.Text, text, StringComparison.Ordinal));
        if (duplicate >= 0) { _characters -= _items[duplicate].Text.Length; _items.RemoveAt(duplicate); }
        _items.Insert(0, new ClipboardItem(Guid.NewGuid(), text, copiedAt)); _characters += text.Length;
        while (_items.Count > MaxItems || _characters > MaxTotalCharacters)
        { _characters -= _items[^1].Text.Length; _items.RemoveAt(_items.Count - 1); }
        return true;
    }

    internal List<ClipboardItem> Find(string query)
    {
        if (query.Length > 256) query = query[..256];
        var words = query.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Take(8).ToArray();
        return _items.Where(item => words.All(word => item.Text.Contains(word, StringComparison.OrdinalIgnoreCase))).ToList();
    }

    internal bool Remove(Guid id)
    {
        var index = _items.FindIndex(item => item.Id == id);
        if (index < 0) return false;
        _characters -= _items[index].Text.Length; _items.RemoveAt(index); return true;
    }

    internal void Clear() { _items.Clear(); _characters = 0; }
}

internal enum ClipboardReadKind { Busy, Skip, Text, TooLarge }
internal readonly record struct ClipboardRead(ClipboardReadKind Kind, uint Sequence, string? Text = null);

// API native dibaca di thread latar; tidak memuat gambar, HTML, atau seluruh riwayat Windows.
internal static class ClipboardNative
{
    private const uint UnicodeText = 13;
    private static readonly uint OwnMarker = RegisterClipboardFormat("NotchMusic.SessionClipboard.v1");
    private static readonly uint Excluded = RegisterClipboardFormat("ExcludeClipboardContentFromMonitorProcessing");
    private static readonly uint HistoryAllowed = RegisterClipboardFormat("CanIncludeInClipboardHistory");
    private static readonly uint CloudAllowed = RegisterClipboardFormat("CanUploadToCloudClipboard");
    internal static uint Sequence => GetClipboardSequenceNumber();

    internal static ClipboardRead Read(IntPtr owner)
    {
        if (!OpenClipboard(owner)) return new(ClipboardReadKind.Busy, 0);
        try
        {
            if (IsClipboardFormatAvailable(OwnMarker) || IsClipboardFormatAvailable(Excluded)
                || (IsClipboardFormatAvailable(HistoryAllowed) && IsZeroDword(HistoryAllowed))
                || !IsClipboardFormatAvailable(UnicodeText)) return new(ClipboardReadKind.Skip, Sequence);
            var handle = GetClipboardData(UnicodeText);
            if (handle == IntPtr.Zero) return new(ClipboardReadKind.Skip, Sequence);
            var count = (int)Math.Min((ulong)GlobalSize(handle) / 2, ClipboardHistoryBuffer.MaxTextCharacters + 1UL);
            if (count == 0) return new(ClipboardReadKind.Skip, Sequence);
            var pointer = GlobalLock(handle);
            if (pointer == IntPtr.Zero) return new(ClipboardReadKind.Skip, Sequence);
            try
            {
                var length = 0;
                while (length < count && Marshal.ReadInt16(pointer, length * 2) != 0) length++;
                if (length > ClipboardHistoryBuffer.MaxTextCharacters || length == count)
                    return new(ClipboardReadKind.TooLarge, Sequence);
                return new(ClipboardReadKind.Text, Sequence, Marshal.PtrToStringUni(pointer, length));
            }
            finally { GlobalUnlock(handle); }
        }
        finally { CloseClipboard(); }
    }

    private static bool IsZeroDword(uint format)
    {
        var handle = GetClipboardData(format);
        if (handle == IntPtr.Zero || (ulong)GlobalSize(handle) < 4) return true;
        var pointer = GlobalLock(handle);
        if (pointer == IntPtr.Zero) return true;
        try { return Marshal.ReadInt32(pointer) == 0; }
        finally { GlobalUnlock(handle); }
    }

    internal static bool Write(IntPtr owner, string text)
    {
        if (text.Length > ClipboardHistoryBuffer.MaxTextCharacters) return false;
        var data = Allocate((text.Length + 1) * 2);
        var marker = Allocate(4); var history = Allocate(4); var cloud = Allocate(4);
        try
        {
            if (data == IntPtr.Zero || marker == IntPtr.Zero || history == IntPtr.Zero || cloud == IntPtr.Zero) return false;
            var pointer = GlobalLock(data);
            if (pointer == IntPtr.Zero) return false;
            try { Marshal.Copy(text.ToCharArray(), 0, pointer, text.Length); Marshal.WriteInt16(pointer, text.Length * 2, 0); }
            finally { GlobalUnlock(data); }
            pointer = GlobalLock(marker);
            if (pointer == IntPtr.Zero) return false;
            try { Marshal.WriteInt32(pointer, 1); } finally { GlobalUnlock(marker); }
            if (!OpenClipboard(owner)) return false;
            try
            {
                if (!EmptyClipboard() || SetClipboardData(UnicodeText, data) == IntPtr.Zero) return false;
                data = IntPtr.Zero; // Clipboard Windows mengambil kepemilikan memory handle.
                if (SetClipboardData(OwnMarker, marker) != IntPtr.Zero) marker = IntPtr.Zero;
                if (SetClipboardData(HistoryAllowed, history) != IntPtr.Zero) history = IntPtr.Zero;
                if (SetClipboardData(CloudAllowed, cloud) != IntPtr.Zero) cloud = IntPtr.Zero;
                return true;
            }
            finally { CloseClipboard(); }
        }
        finally
        {
            if (data != IntPtr.Zero) GlobalFree(data);
            if (marker != IntPtr.Zero) GlobalFree(marker);
            if (history != IntPtr.Zero) GlobalFree(history);
            if (cloud != IntPtr.Zero) GlobalFree(cloud);
        }
    }

    private static IntPtr Allocate(int bytes) => GlobalAlloc(0x0002 | 0x0040, (UIntPtr)bytes);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] internal static extern bool AddClipboardFormatListener(IntPtr window);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] internal static extern bool RemoveClipboardFormatListener(IntPtr window);
    [DllImport("user32.dll")] private static extern uint GetClipboardSequenceNumber();
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool CloseClipboard();
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool EmptyClipboard();
    [DllImport("user32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsClipboardFormatAvailable(uint format);
    [DllImport("user32.dll")] private static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll")] private static extern IntPtr SetClipboardData(uint format, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern uint RegisterClipboardFormat(string name);
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalAlloc(uint flags, UIntPtr bytes);
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalFree(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalLock(IntPtr handle);
    [DllImport("kernel32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GlobalUnlock(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern UIntPtr GlobalSize(IntPtr handle);
}
