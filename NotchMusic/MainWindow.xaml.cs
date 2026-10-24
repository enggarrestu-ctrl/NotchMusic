using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Windows.Foundation;
using Windows.Media.Control;
using Windows.Storage.Streams;
using MediaManager = Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager;
using MediaSession = Windows.Media.Control.GlobalSystemMediaTransportControlsSession;
using Rect = System.Windows.Rect;
using Point = System.Windows.Point;

namespace NotchMusic;

public partial class MainWindow : Window
{
    // Ukuran dalam satuan WPF: otomatis mengikuti skala layar Windows.
    private const double SmallWidth = 128; // Logo 14 + jarak 6 + jam.
    private const double SmallHeight = 34;
    private const double MusicMinWidth = 240;
    private const double MusicHeight = 42;
    private const double CompactInset = 32;
    private const double LargeWidth = 620;
    // Menempel: lengkungan atas memakan MaxFlare di kiri-kanan, jadi notch dilebarkan agar isinya tetap utuh.
    private double ExpandedWidth => LargeWidth + (_docked ? 2 * NotchSurface.MaxFlare : 0);
    private const double LargeHeight = 160;
    // Inset aman dari lengkungan + indikator 30 + jarak 12 + toleransi teks 6.
    private const double CompactChrome = CompactInset * 2 + 30 + 12 + 6;
    private const int LyricScrollMilliseconds = 420;
    private double _compactTextWidth;
    private string? _measuredLyric;
    private double _sizeTargetWidth = SmallWidth;
    private double _sizeTargetHeight = SmallHeight;
    private IntPtr _windowHandle;
    private NativeTrayIcon? _tray;
    private NotchPreferences _preferences = new();
    private bool _repositioning;
    private bool _docked = true;
    private bool _dragArmed;
    private System.Windows.Point _dragPressScreen;
    private double _dragGrabX, _dragGrabY;
    private double _dragDpi;
    private Cursor? _cursorBeforeDrag;
    private HwndSource? _windowSource;
    private bool _positioning;
    private bool _minimizedToTray;
    private bool _awaitingMetadata;
    private bool _reloadLyricsAfterHide;
    private string _lastLyricsTitle = "";
    private string _lastLyricsArtist = "";
    private TranslateTransform[] _lyricShifts = [];

    private MediaManager? _manager;
    private MediaSession? _session;
    private MediaSession? _manualSession;
    private bool _expanded;
    private bool _closed;
    private bool _initializing;
    private bool _commandBusy;
    private ScaleTransform[] _equalizerScales = [];
    private bool _equalizerAnimating;
    private readonly CultureInfo _clockCulture = CultureInfo.GetCultureInfo("id-ID");
    private readonly DispatcherTimer _clockTimer = new();
    private readonly DispatcherTimer _coverRefreshTimer = new() { Interval = TimeSpan.FromMilliseconds(1200) };
    private readonly DispatcherTimer _metadataRetryTimer = new() { Interval = TimeSpan.FromMilliseconds(400) };
    private string? _skipFromTrackKey;
    private long _skipStartedTimestamp;
    private LyricsWindow? _lyricsWindow;
    private readonly DispatcherTimer _lyricTimer = new() { Interval = TimeSpan.FromMilliseconds(100) };
    private CancellationTokenSource? _lyricsCts;
    private readonly SemaphoreSlim _lyricSearchGate = new(1, 1);
    private int _pendingMediaFlags;
    private int _mediaRefreshScheduled;
    private string? _lyricsTrackKey;
    private int _lyricsVersion;
    private int _lyricAnimationVersion;
    private int _transitionVersion;
    private int _hoverMilliseconds = 280;
    private bool _hoverTransitioning;
    private long _hoverStartedTimestamp;
    private int _sizeVersion;
    private bool _morphing;
    private string? _coverFingerprint;
    private int _coverPixels;
    private bool LightAnimations => _preferences.LightAnimations || (RenderCapability.Tier >> 16) < 2;
    // Ikuti refresh monitor, maks 120: lebih halus dari 60 tanpa membebani GPU/CPU seperti 200 Hz.
    private static readonly int DisplayFrames = Math.Clamp(DesktopNative.PrimaryRefreshRate(), 60, 120);
    private int AnimationFrames => LightAnimations ? 30 : DisplayFrames;
    private int _lyricIndex = int.MinValue;
    private LyricsMode _lyricsMode;
    private double _lyricsOffsetSeconds;
    internal double LyricsOffsetSeconds => _lyricsOffsetSeconds;
    private LyricsSearchResult? _lyricsResult;
    private LyricCandidate? _lyricChoice;
    private LyricDocument? _lyricDocument;
    private string _lyricsStatus = "Belum ada lagu.";
    private string _cacheStatus = "";
    private string _cacheIdentity = "";
    private double _cacheDuration;
    private bool _fileBusy;
    private Task _cacheWriteTask = Task.CompletedTask;

    private string _songTitle = "";
    private string _songArtist = "";
    private string _songAlbum = "";
    private bool HasCompactLyrics => !_closed && !_awaitingMetadata && _session is not null && _playing
        && _lyricDocument is { Instrumental: false } doc && doc.Preview.Length > 0;
    private bool AnyMenuOpen => NotchMenu.IsOpen || PlayPauseButton.ContextMenu?.IsOpen == true || _gameMenuOpen;

    // Satu antrean metadata: perubahan yang beruntun digabung agar tidak menumpuk.
    private bool _metadataBusy;
    private bool _metadataPending;
    private int _metadataVersion;

    // Sampel waktu disimpan di memori. Timer progres/lirik tidak membaca ulang sampul atau API.
    private bool _hasTimeline;
    private bool _playing;
    private bool _positionControlEnabled;
    private bool _canSeek;
    private bool _updatingSeek;
    private bool _seekDragging;
    private bool _seekBusy;
    private MediaSession? _dragSession;
    private int _dragVersion;
    private string? _trackKey;
    private double _startSeconds;
    private double _endSeconds;
    private double _seekMinSeconds;
    private double _seekMaxSeconds;
    private double _anchorPosition;
    private long _anchorTimestamp;
    private double _playbackRate = 1;
    private (long Start, long End, long Position, long Updated)? _timelineKey;

    // Progres diperbarui tiap detik saat panel terbuka; animasi hover hanya berjalan sebentar.
    private readonly DispatcherTimer _progressTimer = new()
    {
        Interval = TimeSpan.FromSeconds(1)
    };

    // Timer hanya aktif sebentar setelah mouse meninggalkan notch.
    private readonly DispatcherTimer _collapseTimer = new()
    {
        Interval = TimeSpan.FromMilliseconds(140)
    };

    public MainWindow()
    {
        InitializeComponent();
        NotchViewport.SetPalette((Brush)FindResource("NotchBackground"), (Brush)FindResource("NotchSurface"));
        if (DesignerProperties.GetIsInDesignMode(this)) return;
        foreach (var resource in Resources.Values)
            if (resource is Freezable { CanFreeze: true, IsFrozen: false } value) value.Freeze();
        _preferences = NotchPreferences.Load();
        ApplyTheme(_preferences.Theme);
        _equalizerScales = [EqScale1, EqScale2, EqScale3, EqScale4, EqScale5];
        _lyricShifts = [CompactIncomingShift, CompactOutgoingShift, ExpandedIncomingShift, ExpandedOutgoingShift];
        InitializeClipboardPages();
        _docked = !_preferences.HasPosition || _preferences.DockedToTop;
        SyncNotchShape(false);
        // Pilihan Romaji pada lagu sebelumnya tidak boleh menjadi filter untuk seluruh pemutar.
        _lyricsMode = LyricsMode.Auto;
        Topmost = _preferences.PinOnTop;
        UpdatePinButton();

        Loaded += Window_Loaded;
        SourceInitialized += Window_SourceInitialized;
        Closed += Window_Closed;
        SizeChanged += (_, _) => PositionWindow();
        DpiChanged += (_, _) => OnUi(() =>
        {
            if (_closed || _minimizedToTray) return;
            if (_repositioning) { _measuredLyric = null; return; }
            SnapNotchSize(); _measuredLyric = null; RefreshNotchSize();
            _coverFingerprint = null; QueueMetadata();
        });
        _collapseTimer.Tick += CollapseTimer_Tick;
        // Menu klik kanan dibuka WPF saat tombol kanan dilepas dan belum ditangani.
        PreviewMouseRightButtonUp += (_, e) => { if (GameLocked) e.Handled = true; };
        NotchViewport.ContextMenuOpening += (_, e) => { if (GameLocked) e.Handled = true; };
        _coverRefreshTimer.Tick += (_, _) =>
        {
            _coverRefreshTimer.Stop(); // Satu pembacaan tambahan sesudah pergantian lagu, bukan polling terus-menerus.
            if (!_closed && !_minimizedToTray) QueueMetadata();
        };
        _metadataRetryTimer.Tick += (_, _) =>
        {
            _metadataRetryTimer.Stop();
            if (!_closed && !_minimizedToTray && _skipFromTrackKey is not null) QueueMetadata();
        };
        _progressTimer.Tick += ProgressTimer_Tick;
        _clockTimer.Tick += ClockTimer_Tick;
        _lyricTimer.Tick += LyricTimer_Tick;
        SystemParameters.StaticPropertyChanged += SystemParameters_Changed;
        UpdateClock();
        UpdateCompactState();
        PositionWindow();
    }

    private void ClockTimer_Tick(object? sender, EventArgs e) => UpdateClock();

    private void UpdateClock()
    {
        if (_closed || _minimizedToTray) { _clockTimer.Stop(); return; }
        var now = DateTime.Now; // Mengikuti jam dan zona waktu PC.
        CompactClock.Text = ClockFull.Text = now.ToString("HH:mm", _clockCulture);
        FullDate.Text = _clockCulture.TextInfo.ToTitleCase(now.ToString("dddd, dd MMMM yyyy", _clockCulture));
        _clockTimer.Stop();
        _clockTimer.Interval = TimeSpan.FromMilliseconds((60 - now.Second) * 1000 - now.Millisecond + 50);
        _clockTimer.Start(); // Cukup memperbarui di pergantian menit.
    }

    private async void Window_Loaded(object sender, RoutedEventArgs e)
    {
        _ = StartNotificationsAsync();
        StartPadWatch();
        await InitializeMediaAsync();
        ScheduleTrim(); // Sisa inisialisasi (WinRT, XAML, JIT) tidak perlu ditahan.
    }

    // Meminta daftar pemutar yang terhubung ke kontrol media Windows.
    private async Task InitializeMediaAsync()
    {
        if (_initializing || _closed) return;
        if (_manager is not null)
        {
            SelectSession(forceRefresh: true);
            return;
        }

        _initializing = true;
        try
        {
            var manager = await MediaManager.RequestAsync();
            if (_closed) return;
            _manager = manager;
            _manager.CurrentSessionChanged += Manager_CurrentSessionChanged;
            _manager.SessionsChanged += Manager_SessionsChanged;
            SelectSession(forceRefresh: true);
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            SongTitle.Text = "Belum bisa membaca media";
            ArtistName.Text = "Klik tombol sumber lalu Baca ulang media";
        }
        finally { _initializing = false; }
    }

    // Event Windows dapat datang dari thread lain. Perubahan UI dijadwalkan di thread UI.
    private void OnUi(Action action)
    {
        if (_closed || Dispatcher.HasShutdownStarted) return;
        Dispatcher.BeginInvoke(DispatcherPriority.Background, new Action(() =>
        {
            if (!_closed) action();
        }));
    }

    private void Manager_CurrentSessionChanged(MediaManager sender, CurrentSessionChangedEventArgs args)
        => OnUi(() => SelectSession());

    private void Manager_SessionsChanged(MediaManager sender, SessionsChangedEventArgs args)
        => OnUi(() => SelectSession());

    private void SelectSession(bool forceRefresh = false)
    {
        if (_manager is null || _closed) return;
        try
        {
            var sessions = _manager.GetSessions();
            if (_manualSession is not null && !sessions.Any(s => Equals(s, _manualSession)))
                _manualSession = null;

            var next = _manualSession ?? _manager.GetCurrentSession() ?? sessions.FirstOrDefault();
            if (Equals(next, _session))
            {
                UpdatePlayback();
                if (forceRefresh) QueueMetadata();
                return;
            }

            DetachSession();
            _session = next;
            _metadataVersion++; // Hasil pembacaan sesi lama tidak boleh menimpa sesi baru.
            ResetDisplay();

            if (_session is not null)
            {
                _session.MediaPropertiesChanged += Session_MediaPropertiesChanged;
                _session.PlaybackInfoChanged += Session_PlaybackInfoChanged;
                _session.TimelinePropertiesChanged += Session_TimelinePropertiesChanged;
            }
            UpdatePlayback();
            if (_session is not null) QueueMetadata();
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            StatusText.Text = "Coba klik kanan lalu Baca ulang media";
        }
    }

    private void Session_MediaPropertiesChanged(MediaSession sender, MediaPropertiesChangedEventArgs args)
        => QueueMediaRefresh(1 | 4);

    private void Session_PlaybackInfoChanged(MediaSession sender, PlaybackInfoChangedEventArgs args)
        => QueueMediaRefresh(2);

    private void Session_TimelinePropertiesChanged(MediaSession sender, TimelinePropertiesChangedEventArgs args)
        => QueueMediaRefresh(4);

    private void QueueMediaRefresh(int flags)
    {
        // Gabungkan burst event; jangan menumpuk callback/referensi sesi di dispatcher.
        Interlocked.Or(ref _pendingMediaFlags, flags);
        if (Interlocked.Exchange(ref _mediaRefreshScheduled, 1) != 0) return;
        OnUi(() =>
        {
            var pending = Interlocked.Exchange(ref _pendingMediaFlags, 0);
            try
            {
                if (_minimizedToTray) return; // Restore membaca sesi dan posisi terbaru.
                if ((pending & 1) != 0) QueueMetadata();
                if ((pending & 2) != 0) UpdatePlayback();
                else if ((pending & 4) != 0) RefreshTimeline();
            }
            finally
            {
                Interlocked.Exchange(ref _mediaRefreshScheduled, 0);
                // Event dapat datang ketika callback ini sedang bekerja; jangan sampai terlewat.
                if (Volatile.Read(ref _pendingMediaFlags) != 0) QueueMediaRefresh(0);
            }
        });
    }

    private void QueueMetadata()
    {
        if (_closed || _minimizedToTray || _session is null) return;
        _metadataPending = true;
        _metadataVersion++;
        if (!_metadataBusy) _ = ReadMetadataLoopAsync();
    }

    private bool IsCurrent(MediaSession session, int version)
        => !_closed && !_minimizedToTray && Equals(session, _session) && version == _metadataVersion;

    private static string MediaTrackKey(GlobalSystemMediaTransportControlsSessionMediaProperties media)
        => $"{media.Title}\u001F{media.Artist}\u001F{media.AlbumTitle}";

    private void ShowTrackTransition()
    {
        _awaitingMetadata = true;
        _coverRefreshTimer.Stop();
        _lyricTimer.Stop(); _lyricsVersion++; _lyricsCts?.Cancel();
        _lyricsTrackKey = null; // Pulihkan lirik dari cache juga bila perintah skip tidak mengganti lagu.
        _songTitle = _songArtist = _songAlbum = "";
        _lyricDocument = null; _lyricsResult = null; _lyricChoice = null;
        _lyricsStatus = "Menunggu metadata lagu baru…"; _cacheStatus = "";
        LyricsButton.IsEnabled = false;
        SongTitle.Text = "Mengganti lagu…"; ArtistName.Text = "Menunggu pemutar";
        CoverImage.Source = null; CoverFallback.Visibility = Visibility.Visible;
        SetLyricMessage("Menunggu lagu baru…", "Lirik");
        UpdateLyricsWindow();
    }

    // Baca sampul pada setiap pembaruan metadata, termasuk ketika judul lagunya tetap sama.
    private async Task ReadMetadataLoopAsync()
    {
        _metadataBusy = true;
        try
        {
            while (_metadataPending && !_closed)
            {
                _metadataPending = false;
                var session = _session;
                var version = _metadataVersion;
                if (session is null) continue;

                try
                {
                    // Gabungkan rentetan metadata judul/artis/album yang belum selesai diperbarui.
                    await Task.Delay(200);
                    if (!IsCurrent(session, version)) continue;
                    var media = await session.TryGetMediaPropertiesAsync();
                    if (!IsCurrent(session, version)) continue;

                    var trackKey = MediaTrackKey(media);
                    if (_skipFromTrackKey is string previousKey && trackKey == previousKey
                        && Stopwatch.GetElapsedTime(_skipStartedTimestamp) < TimeSpan.FromSeconds(4))
                    {
                        // TrySkip dapat selesai sebelum pemutar mengirim identitas lagu barunya.
                        _metadataRetryTimer.Stop(); _metadataRetryTimer.Start();
                        continue;
                    }
                    var trackChanged = _trackKey != trackKey;
                    if (trackChanged)
                    {
                        if (!_awaitingMetadata) ShowTrackTransition();
                        await Task.Delay(250);
                        if (!IsCurrent(session, version)) continue;
                        var confirmed = await session.TryGetMediaPropertiesAsync();
                        if (!IsCurrent(session, version)) continue;
                        if (MediaTrackKey(confirmed) != trackKey) { QueueMetadata(); continue; }
                        media = confirmed;
                    }
                    var unchangedAfterSkip = _skipFromTrackKey is not null && trackKey == _skipFromTrackKey;
                    _skipFromTrackKey = null; _metadataRetryTimer.Stop();
                    if (trackChanged)
                    {
                        _trackKey = trackKey;
                        CoverImage.Source = null;
                        CoverFallback.Visibility = Visibility.Visible;
                        _coverRefreshTimer.Stop();
                        _coverRefreshTimer.Start(); // Beberapa pemutar mengirim cover sedikit setelah judul.
                        ResetTimeline();
                        RefreshTimeline();
                    }
                    SongTitle.Text = string.IsNullOrWhiteSpace(media.Title) ? "Media sedang aktif" : media.Title;
                    ArtistName.Text = media.Artist ?? "";
                    _songTitle = media.Title ?? "";
                    _songArtist = media.Artist ?? "";
                    _songAlbum = media.AlbumTitle ?? "";
                    _awaitingMetadata = false;
                    if (unchangedAfterSkip) StatusText.Text = "Pemutar masih melaporkan lagu yang sama";
                    // Metadata lagu yang sama boleh diperbarui tanpa mereset baris aktif.
                    // Reset di sini membuat pergantian baris dianggap sebagai render pertama.
                    LyricsButton.IsEnabled = !string.IsNullOrWhiteSpace(_songTitle);
                    EnsureLyricsTrack();
                    RenderLyrics(); UpdateLyricTimer();
                    UpdateLyricsWindow();
                    var pixels = Math.Clamp((int)Math.Ceiling(48 * VisualTreeHelper.GetDpi(this).DpiScaleX), 48, 144);
                    var previousCover = CoverImage.Source is not null && _coverPixels == pixels ? _coverFingerprint : null;
                    var cover = await ReadCoverAsync(media.Thumbnail, pixels, previousCover);
                    // Versi mencakup perubahan lagu/metadata dan sesi. Hasil pembacaan lama tidak boleh dipasang.
                    if (!IsCurrent(session, version) || _trackKey != trackKey) continue;
                    if (!cover.Unchanged)
                    {
                        CoverImage.Source = cover.Image;
                        CoverFallback.Visibility = cover.Image is null ? Visibility.Visible : Visibility.Collapsed;
                    }
                    _coverFingerprint = cover.Fingerprint; _coverPixels = pixels;
                }
                catch (Exception ex)
                {
                    Debug.WriteLine(ex);
                    if (IsCurrent(session, version))
                    {
                        StatusText.Text = "Metadata belum tersedia dari pemutar";
                        if (_skipFromTrackKey is not null && Stopwatch.GetElapsedTime(_skipStartedTimestamp) < TimeSpan.FromSeconds(4))
                        { _metadataRetryTimer.Stop(); _metadataRetryTimer.Start(); }
                        else if (_awaitingMetadata)
                        {
                            _skipFromTrackKey = null; _metadataRetryTimer.Stop();
                            SongTitle.Text = "Metadata belum tersedia"; ArtistName.Text = "Klik kanan → Baca ulang media";
                        }
                    }
                }
            }
        }
        finally { _metadataBusy = false; }
    }

    private sealed record CoverRead(BitmapSource? Image, string? Fingerprint, bool Unchanged = false);

    private static Task<CoverRead> ReadCoverAsync(IRandomAccessStreamReference? thumbnail, int pixels, string? previous)
        => Task.Run(async () =>
    {
        if (thumbnail is null) return new CoverRead(null, null);
        try
        {
            using var input = await thumbnail.OpenReadAsync();
            if (input.Size == 0 || input.Size > 4 * 1024 * 1024) return new CoverRead(null, null);
            using var stream = input.AsStreamForRead();
            // Hash dibaca secara streaming. Metadata duplikat tidak mendekode gambar yang sama lagi.
            var hash = Convert.ToHexString(await System.Security.Cryptography.SHA256.HashDataAsync(stream));
            if (hash == previous) return new CoverRead(null, hash, true);
            stream.Position = 0;
            var decoder = BitmapDecoder.Create(stream, BitmapCreateOptions.DelayCreation, BitmapCacheOption.None);
            var frame = decoder.Frames[0];
            var sourceWidth = frame.PixelWidth; var sourceHeight = frame.PixelHeight;
            if (sourceWidth <= 0 || sourceHeight <= 0 || (long)sourceWidth * sourceHeight > 64000000)
                return new CoverRead(null, null);
            stream.Position = 0;
            var image = new BitmapImage();
            image.BeginInit();
            image.CacheOption = BitmapCacheOption.OnLoad;
            // Kotak sampul memakai UniformToFill: sisi terpendek yang mengisi kotak, jadi itu yang dibatasi.
            // Membatasi sisi terpanjang membuat thumbnail 16:9 (YouTube) diperbesar ~1,8x dan pecah.
            if (sourceWidth >= sourceHeight) image.DecodePixelHeight = Math.Min(pixels, sourceHeight);
            else image.DecodePixelWidth = Math.Min(pixels, sourceWidth);
            image.StreamSource = stream;
            image.EndInit();
            var converted = new FormatConvertedBitmap(image, PixelFormats.Pbgra32, null, 0);
            var stride = checked(converted.PixelWidth * 4);
            var buffer = new byte[checked(stride * converted.PixelHeight)];
            converted.CopyPixels(buffer, stride, 0);
            // Simpan piksel kecil saja, tanpa menahan decoder/stream/metadata gambar asal.
            var detached = BitmapSource.Create(converted.PixelWidth, converted.PixelHeight, 96, 96,
                PixelFormats.Pbgra32, null, buffer, stride);
            detached.Freeze();
            return new CoverRead(detached, hash);
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            return new CoverRead(null, null); // Simbol musik jika sampul tidak tersedia.
        }
    });

    private void UpdatePlayback()
    {
        PreviousButton.IsEnabled = PlayPauseButton.IsEnabled = NextButton.IsEnabled = false;
        StopMenuItem.IsEnabled = false;
        if (_closed) return;
        if (_session is null)
        {
            _playing = false;
            _positionControlEnabled = false;
            ResetTimeline();
            UpdateLyricTimer();
            UpdateCompactState();
            return;
        }
        try
        {
            var info = _session.GetPlaybackInfo();
            // Bekukan perkiraan lama sebelum mengganti status atau kecepatan.
            _anchorPosition = EstimatedPosition();
            _anchorTimestamp = Stopwatch.GetTimestamp();
            var playing = info.PlaybackStatus == GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing;
            _playing = playing;
            _playbackRate = info.PlaybackRate ?? 1;
            if (!double.IsFinite(_playbackRate)) _playbackRate = 1;
            var controls = info.Controls;
            _positionControlEnabled = controls.IsPlaybackPositionEnabled;
            var ready = !_commandBusy && !_seekBusy;
            PlayPauseIcon.Data = (Geometry)FindResource(playing ? "PauseIconGeometry" : "PlayIconGeometry");
            PlayPauseButton.ToolTip = playing ? "Jeda" : "Putar";
            AutomationProperties.SetName(PlayPauseButton, playing ? "Jeda musik" : "Putar musik");
            PreviousButton.IsEnabled = ready && controls.IsPreviousEnabled;
            NextButton.IsEnabled = ready && controls.IsNextEnabled;
            PlayPauseButton.IsEnabled = ready && (controls.IsPlayPauseToggleEnabled ||
                (playing ? controls.IsPauseEnabled : controls.IsPlayEnabled));
            StopMenuItem.IsEnabled = ready && controls.IsStopEnabled;
            StatusText.Text = "";
            RefreshTimeline();
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            _playing = false;
            _positionControlEnabled = false;
            ResetTimeline();
        }
        RenderLyrics();
        UpdateLyricTimer();
        UpdateCompactState();
    }

    private void UpdateCompactState()
    {
        // Lirik belum tersedia/tidak ditemukan, pause, stop, atau instrumental: jam saja.
        // Pesan WhatsApp baru menutupi keduanya sebentar.
        var notice = _notice is not null;
        var music = HasCompactLyrics && !notice;
        CompactNotice.Visibility = notice ? Visibility.Visible : Visibility.Collapsed;
        CompactIdle.Visibility = music || notice ? Visibility.Collapsed : Visibility.Visible;
        CompactMusic.Visibility = music ? Visibility.Visible : Visibility.Collapsed;
        SetEqualizerAnimation(music && !_expanded && !_minimizedToTray && SystemParameters.ClientAreaAnimation);
        RefreshNotchSize();
    }

    private void SetEqualizerAnimation(bool animate)
    {
        if (_equalizerAnimating == animate) return;
        _equalizerAnimating = animate;
        for (var i = 0; i < _equalizerScales.Length; i++)
        {
            var scale = _equalizerScales[i];
            scale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
            scale.ScaleY = 0.35 + (i % 3) * 0.2;
            if (!animate) continue;
            // Lima transform kecil, 20 fps; berhenti saat panel terbuka atau musik dijeda.
            var pulse = new DoubleAnimation(0.18 + i * 0.04, 1, TimeSpan.FromMilliseconds(340 + i * 71))
            {
                AutoReverse = true, RepeatBehavior = RepeatBehavior.Forever,
                EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut }
            };
            Timeline.SetDesiredFrameRate(pulse, LightAnimations ? 12 : 20);
            scale.BeginAnimation(ScaleTransform.ScaleYProperty, pulse, HandoffBehavior.SnapshotAndReplace);
        }
    }

    private double EstimatedPosition()
    {
        if (!_hasTimeline) return 0;
        var elapsed = _playing ? Stopwatch.GetElapsedTime(_anchorTimestamp).TotalSeconds : 0;
        return Math.Clamp(_anchorPosition + elapsed * _playbackRate, _startSeconds, _endSeconds);
    }

    // Dipanggil oleh event Windows dan saat membuka panel, bukan setiap detik.
    private void RefreshTimeline()
    {
        if (_closed) return;
        if (_session is null) { ResetTimeline(); return; }
        try
        {
            var timeline = _session.GetTimelineProperties();
            var start = timeline.StartTime.TotalSeconds;
            var end = timeline.EndTime.TotalSeconds;
            if (end <= start) { ResetTimeline(); return; }

            var key = (timeline.StartTime.Ticks, timeline.EndTime.Ticks,
                timeline.Position.Ticks, timeline.LastUpdatedTime.UtcTicks);
            var changed = !_hasTimeline || _timelineKey != key;
            _startSeconds = start;
            _endSeconds = end;
            _hasTimeline = true;
            if (changed)
            {
                var now = DateTimeOffset.UtcNow;
                var age = timeline.LastUpdatedTime.Year >= 2000
                    ? Math.Max(0, (now - timeline.LastUpdatedTime).TotalSeconds) : 0;
                _anchorPosition = Math.Clamp(timeline.Position.TotalSeconds +
                    (_playing ? age * _playbackRate : 0), start, end);
                _anchorTimestamp = Stopwatch.GetTimestamp();
                _timelineKey = key;
            }
            // Nilai Position tetap menunjuk waktu LastUpdatedTime, bukan waktu getter.
            // Sampel yang sama tidak boleh membuat posisi mundur saat pause/resume.
            _seekMinSeconds = Math.Max(start, timeline.MinSeekTime.TotalSeconds);
            _seekMaxSeconds = Math.Min(end, timeline.MaxSeekTime.TotalSeconds);
            _canSeek = _positionControlEnabled && _seekMaxSeconds > _seekMinSeconds;
            UpdateSeekAvailability();
            RenderProgress();
            UpdateProgressTimer();
            RenderLyrics();
            UpdateLyricTimer();
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            ResetTimeline();
        }
    }

    private void ResetTimeline()
    {
        _progressTimer.Stop();
        NotchViewport.SetProgress(0, false);
        _lyricTimer.Stop();
        _hasTimeline = _canSeek = false;
        _timelineKey = null;
        _startSeconds = _endSeconds = _anchorPosition = 0;
        _updatingSeek = true;
        try
        {
            SeekSlider.IsEnabled = false;
            SeekSlider.Value = 0;
            SeekSlider.Maximum = 1;
        }
        finally { _updatingSeek = false; }
        ElapsedText.Text = DurationText.Text = "--:--";
        SeekSlider.ToolTip = "Waktu belum tersedia dari pemutar";
        ProgressPath.Visibility = FullProgress.Visibility = Visibility.Collapsed;
    }

    private void UpdateSeekAvailability()
    {
        SeekSlider.IsEnabled = _hasTimeline && _canSeek && !_commandBusy && !_seekBusy;
        SeekSlider.ToolTip = !_hasTimeline ? "Waktu belum tersedia dari pemutar"
            : _canSeek ? "Geser bulatan, klik bar, atau gunakan tombol panah"
            : "Waktu tersedia; pemutar ini belum mendukung geser posisi";
    }

    private void UpdateProgressTimer()
    {
        var position = EstimatedPosition();
        var canAdvance = _playbackRate > 0 ? position < _endSeconds
            : _playbackRate < 0 && position > _startSeconds;
        // Notch tertutup juga butuh detik berjalan untuk garis progres di tepinya.
        if (!_closed && !_minimizedToTray && (!_expanded || _expandedPage == 0) && _hasTimeline && _playing && !_seekDragging && canAdvance)
            _progressTimer.Start();
        else
            _progressTimer.Stop();
    }

    private void ProgressTimer_Tick(object? sender, EventArgs e)
    {
        RenderProgress();
        UpdateProgressTimer();
    }

    private void RenderProgress(double? previewElapsed = null)
    {
        if (_closed) return;
        if (!_hasTimeline) { NotchViewport.SetProgress(0, false); return; }
        if (_seekDragging && previewElapsed is null) return;
        var duration = _endSeconds - _startSeconds;
        var elapsed = Math.Clamp(previewElapsed ?? EstimatedPosition() - _startSeconds, 0, duration);
        _updatingSeek = true;
        try
        {
            SeekSlider.Maximum = duration;
            SeekSlider.Value = elapsed;
        }
        finally { _updatingSeek = false; }
        ElapsedText.Text = FormatTime(elapsed);
        DurationText.Text = FormatTime(duration);

        var fraction = elapsed / duration;
        NotchViewport.SetProgress(fraction, !_expanded);
        FullProgress.Visibility = fraction >= 1 ? Visibility.Visible : Visibility.Collapsed;
        ProgressPath.Visibility = fraction > 0 && fraction < 1 ? Visibility.Visible : Visibility.Collapsed;
        if (fraction > 0 && fraction < 1)
        {
            var angle = fraction * 2 * Math.PI;
            // Pusat/radius mengikuti bentuk cincin di XAML agar tetap sepusat saat diperkecil.
            var start = ProgressFigure.StartPoint;
            var radius = ProgressArc.Size.Width;
            ProgressArc.Point = new System.Windows.Point(start.X + radius * Math.Sin(angle),
                start.Y + radius * (1 - Math.Cos(angle)));
            ProgressArc.IsLargeArc = fraction > 0.5;
        }
    }

    private static string FormatTime(double seconds)
    {
        var time = TimeSpan.FromSeconds(Math.Max(0, Math.Floor(seconds)));
        return time.TotalHours >= 1
            ? $"{(int)time.TotalHours}:{time.Minutes:00}:{time.Seconds:00}"
            : $"{time.Minutes}:{time.Seconds:00}";
    }

    private void SeekSlider_DragStarted(object sender, DragStartedEventArgs e)
    {
        _seekDragging = true;
        _dragSession = _session;
        _dragVersion = _metadataVersion;
        _progressTimer.Stop();
        _collapseTimer.Stop();
    }

    private async void SeekSlider_DragCompleted(object sender, DragCompletedEventArgs e)
    {
        var value = SeekSlider.Value;
        var valid = !e.Canceled && _dragSession is not null && IsCurrent(_dragSession, _dragVersion);
        _seekDragging = false;
        _dragSession = null;
        if (valid) await SeekToAsync(value);
        RenderProgress();
        RenderLyrics();
        UpdateProgressTimer();
        UpdateLyricTimer();
        if (!_closed && !IsMouseOver && !AnyMenuOpen) _collapseTimer.Start();
    }

    private async void SeekSlider_ValueChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        if (_updatingSeek || !IsLoaded || _closed || !_hasTimeline) return;
        RenderProgress(e.NewValue);
        // Selama drag hanya pratinjau. Kirim satu perintah ketika mouse dilepas.
        if (!_seekDragging) await SeekToAsync(e.NewValue);
    }

    private async Task SeekToAsync(double elapsed)
    {
        var session = _session;
        var version = _metadataVersion;
        if (session is null || _closed || !_hasTimeline || !_canSeek || _seekBusy || _commandBusy) return;
        var target = Math.Clamp(_startSeconds + elapsed, _seekMinSeconds, _seekMaxSeconds);
        _seekBusy = true;
        UpdatePlayback();
        string? error = null;
        try
        {
            // API meminta ticks (100 ns). Detik harus dikonversi lebih dahulu.
            var accepted = await session.TryChangePlaybackPositionAsync(TimeSpan.FromSeconds(target).Ticks);
            if (!IsCurrent(session, version)) return;
            if (accepted)
            {
                _anchorPosition = target;
                _anchorTimestamp = Stopwatch.GetTimestamp();
            }
            else error = "Pemutar belum menerima perubahan posisi";
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            error = "Posisi belum berubah; coba dari aplikasi pemutar";
        }
        finally
        {
            _seekBusy = false;
            if (!_closed)
            {
                UpdatePlayback();
                if (error is not null && IsCurrent(session, version)) StatusText.Text = error;
            }
        }
    }

    private async Task SendCommandAsync(Func<MediaSession, IAsyncOperation<bool>> command, bool changesTrack = false)
    {
        var session = _session;
        if (session is null || _commandBusy || _seekBusy || _closed) return;
        _commandBusy = true;
        if (changesTrack)
        {
            _metadataVersion++; _metadataPending = false;
            _metadataRetryTimer.Stop();
            _skipFromTrackKey = _trackKey ?? "";
            _skipStartedTimestamp = Stopwatch.GetTimestamp();
            ShowTrackTransition();
        }
        UpdatePlayback();
        string? error = null;
        try
        {
            if (!await command(session)) error = "Pemutar belum menerima perintah ini";
        }
        catch (Exception ex)
        {
            Debug.WriteLine(ex);
            error = "Pemutar tidak merespons; coba putar dari aplikasinya";
        }
        finally
        {
            _commandBusy = false;
            if (!_closed)
            {
                UpdatePlayback();
                if (Equals(session, _session))
                {
                    if (changesTrack)
                    {
                        if (error is not null) { _skipFromTrackKey = null; _metadataRetryTimer.Stop(); }
                        QueueMetadata();
                    }
                    if (error is not null) StatusText.Text = error;
                }
            }
        }
    }

    private async void Previous_Click(object sender, RoutedEventArgs e)
        => await SendCommandAsync(s => s.TrySkipPreviousAsync(), changesTrack: true);

    private async void Next_Click(object sender, RoutedEventArgs e)
        => await SendCommandAsync(s => s.TrySkipNextAsync(), changesTrack: true);

    private async void Stop_Click(object sender, RoutedEventArgs e)
        => await SendCommandAsync(s => s.TryStopAsync());

    private async void PlayPause_Click(object sender, RoutedEventArgs e)
        => await SendCommandAsync(s =>
        {
            var info = s.GetPlaybackInfo();
            if (info.Controls.IsPlayPauseToggleEnabled) return s.TryTogglePlayPauseAsync();
            return info.PlaybackStatus == GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing
                ? s.TryPauseAsync() : s.TryPlayAsync();
        });

    private static string FriendlySource(string id)
    {
        var value = id.ToLowerInvariant();
        if (value.Contains("spotify")) return "Spotify";
        if (value.Contains("msedge")) return "Microsoft Edge";
        if (value.Contains("chrome")) return "Google Chrome";
        if (value.Contains("brave")) return "Brave";
        if (value.Contains("firefox")) return "Firefox";
        if (value.Contains("youtube")) return "YouTube / YouTube Music";
        return string.IsNullOrWhiteSpace(id) ? "Pemutar media" : id;
    }

    private void ResetDisplay()
    {
        _coverRefreshTimer.Stop();
        _metadataRetryTimer.Stop(); _skipFromTrackKey = null;
        _awaitingMetadata = false;
        if (_minimizedToTray)
        {
            _lyricDocument = null; _lyricsResult = null; _lyricChoice = null; _lyricsTrackKey = null;
        }
        _playing = false;
        _positionControlEnabled = false;
        _trackKey = null;
        _songTitle = _songArtist = _songAlbum = "";
        UpdateCompactState();
        LyricsButton.IsEnabled = false;
        EnsureLyricsTrack();
        UpdateLyricsWindow();
        UpdateLyricTimer();
        ResetTimeline();
        SongTitle.Text = _session is null ? "Belum ada musik" : "Membaca lagu…";
        ArtistName.Text = "Putar lagu di pemutar favoritmu";
        CoverImage.Source = null;
        CoverFallback.Visibility = Visibility.Visible;
        StatusText.Text = "";
        PlayPauseIcon.Data = (Geometry)FindResource("PlayIconGeometry");
    }

    private void PositionWindow()
    {
        if (_closed || _repositioning || _positioning || _minimizedToTray || _morphing) return;
        if (_windowHandle == IntPtr.Zero)
        {
            var area = SystemParameters.WorkArea;
            Left = area.Left + (area.Width - Width) / 2; Top = 0;
            return;
        }
        // Jendela adalah panggung yang bisa tidak simetris terhadap notch (dekat tepi layar),
        // jadi letakkan ulang panggung + clip bersama-sama, bukan memusatkan HWND saja.
        // Bila sudah tepat, SetHostBounds tidak memanggil SetWindowPos sama sekali.
        SnapNotchSize();
    }

    private DesktopNative.NativeRect ActiveScreenArea()
        => DesktopNative.ScreenArea(_windowHandle, _preferences.HasPosition,
            _preferences.CenterX, _preferences.TopY);

    private void Window_SourceInitialized(object? sender, EventArgs e)
    {
        _windowHandle = new WindowInteropHelper(this).Handle;
        DesktopNative.HideFromAltTab(_windowHandle);
        // Jendela topmost lain yang diaktifkan bisa naik ke atas notch; naikkan lagi setiap jendela aktif berganti.
        _foregroundChanged = (_, _, _, _, _, _, _) =>
        { if (Topmost && !_closed && !_minimizedToTray) DesktopNative.KeepOnTop(_windowHandle); };
        _foregroundHook = DesktopNative.WatchForeground(_foregroundChanged);
        _windowSource = HwndSource.FromHwnd(_windowHandle);
        _windowSource?.AddHook(Window_Hook);
        StartClipboardListener();
        _tray = new NativeTrayIcon(_windowHandle,
            () => OnUi(RestoreFromTray),
            () => OnUi(() => { RestoreFromTray(); NotchMenu.PlacementTarget = Shell;
                NotchMenu.Placement = PlacementMode.MousePoint; NotchMenu.IsOpen = true; }),
            () => OnUi(() => { if (_minimizedToTray) RestoreFromTray(); }));
        SnapNotchSize();
        OnUi(() => { _measuredLyric = null; RefreshNotchSize(); PositionWindow(); });
    }

    private static bool IsPlayerControl(DependencyObject? source)
    {
        while (source is not null)
        {
            if (source is ButtonBase or Slider or Thumb or TextBoxBase or PasswordBox or Selector or ScrollBar) return true;
            source = source is Visual ? VisualTreeHelper.GetParent(source) : LogicalTreeHelper.GetParent(source);
        }
        return false;
    }

    // Saat game layar penuh aktif, klik tak sengaja tidak boleh membuka/menekan notch.
    // Klik kiri 2x membuka kunci sampai notch menutup lagi.
    // Status dikunci saat mouse masuk, selagi game masih di depan. Dicek ulang saat klik tidak bisa:
    // begitu ditekan, notch bisa menjadi jendela depan sehingga game dianggap sudah tidak aktif.
    private bool _gameUnlocked;
    private bool _gameDragging; // Ikon game sedang diseret (lihat GameFeature).
    private bool _gameLatched;
    private bool GameLocked => _gameLatched && !_gameUnlocked;

    private void Window_DragPressed(object sender, MouseButtonEventArgs e)
    {
        if (GameLocked)
        {
            e.Handled = true; // Klik tunggal tidak diteruskan ke tombol mana pun.
            if (e.ClickCount == 2) { _gameUnlocked = true; _collapseTimer.Stop(); SetExpanded(true); }
            return;
        }
        if (_closed || _minimizedToTray || _seekDragging || _repositioning
            || AnyMenuOpen || IsPlayerControl(e.OriginalSource as DependencyObject)
            || !DesktopNative.TryGetCursor(out _dragPressScreen)) return;
        _dragArmed = CaptureMouse();
        if (!_dragArmed) return;
        e.Handled = true;
        _collapseTimer.Stop();
    }

    private void Window_DragMoved(object sender, MouseEventArgs e)
    {
        if ((!_dragArmed && !_repositioning) || _closed || _minimizedToTray) return;
        if (e.LeftButton != MouseButtonState.Pressed) { FinishWindowDrag(); return; }
        if (!DesktopNative.TryGetCursor(out var point)) return;
        if (!_repositioning)
        {
            var dpi = VisualTreeHelper.GetDpi(this);
            if (Math.Abs(point.X - _dragPressScreen.X) < SystemParameters.MinimumHorizontalDragDistance * dpi.DpiScaleX
                && Math.Abs(point.Y - _dragPressScreen.Y) < SystemParameters.MinimumVerticalDragDistance * dpi.DpiScaleY) return;
            // Dipindah dalam bentuk kecil: tutup panel seketika sebelum mulai menyeret.
            if (_expanded || _hoverTransitioning)
            { _collapseTimer.Stop(); SetExpanded(false); SetTransitionImmediately(); SnapNotchSize(); }
            if (!DesktopNative.GetWindowRect(_windowHandle, out var window)) return;
            _repositioning = true;
            _dragDpi = dpi.DpiScaleX;
            _cursorBeforeDrag = Mouse.OverrideCursor;
            Mouse.OverrideCursor = Cursors.Arrow;
            var visible = FreezeForWindowDrag(window);
            // Tetap menahan titik yang diklik, termasuk jika hover belum selesai.
            _dragGrabX = Math.Clamp((_dragPressScreen.X - visible.Left) / visible.Width, 0, 1);
            _dragGrabY = Math.Clamp((_dragPressScreen.Y - visible.Top) / visible.Height, 0, 1);
            StopLyricScroll();
        }
        e.Handled = true;
        MoveWindowWithPointer(point);
    }

    private void Window_DragReleased(object sender, MouseButtonEventArgs e)
    {
        if (!_dragArmed && !_repositioning) return;
        if (_repositioning && DesktopNative.TryGetCursor(out var point)) MoveWindowWithPointer(point);
        e.Handled = true;
        FinishWindowDrag();
    }

    private void Window_DragCaptureLost(object sender, MouseEventArgs e)
    {
        if (!IsMouseCaptured && (_dragArmed || _repositioning)) FinishWindowDrag();
    }

    private Rect FreezeForWindowDrag(DesktopNative.NativeRect window)
    {
        var dpi = VisualTreeHelper.GetDpi(this);
        var visible = NotchViewport.VisibleBounds;
        var screen = new Rect(window.Left + visible.X * dpi.DpiScaleX,
            window.Top + visible.Y * dpi.DpiScaleY, visible.Width * dpi.DpiScaleX, visible.Height * dpi.DpiScaleY);
        ++_sizeVersion; ++_transitionVersion;
        NotchViewport.AnimateBounds(null);
        NotchViewport.VisibleBounds = new Rect(0, 0, visible.Width, visible.Height);
        _sizeTargetWidth = visible.Width; _sizeTargetHeight = visible.Height;
        _morphing = false;
        // Pindahkan asal clip/HWND tanpa mengubah letak isi yang sedang terlihat.
        var shiftX = ShellShift.X; var shiftY = ShellShift.Y;
        ShellShift.BeginAnimation(TranslateTransform.XProperty, null);
        ShellShift.BeginAnimation(TranslateTransform.YProperty, null);
        ShellShift.X = shiftX; ShellShift.Y = shiftY;
        Canvas.SetLeft(Shell, Canvas.GetLeft(Shell) - visible.X);
        Canvas.SetTop(Shell, Canvas.GetTop(Shell) - visible.Y);
        foreach (var panel in new FrameworkElement[] { CompactPanel, ExpandedHost })
        {
            var opacity = panel.Opacity;
            panel.BeginAnimation(OpacityProperty, null); panel.Opacity = opacity;
            panel.IsHitTestVisible = false;
        }
        var scale = ExpandedScale.ScaleX;
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
        ExpandedScale.ScaleX = ExpandedScale.ScaleY = scale;
        SetHostBounds(screen);
        return screen;
    }

    private void MoveWindowWithPointer(Point point)
    {
        if (!DesktopNative.GetWindowRect(_windowHandle, out var window)) return;
        var area = DesktopNative.ScreenArea(_windowHandle, true, point.X, point.Y);
        var placement = NotchPlacement.Resolve(point.X, point.Y, _dragGrabX, _dragGrabY,
            window.Width, window.Height, area, VisualTreeHelper.GetDpi(this).DpiScaleY, _docked);
        var changed = _docked != placement.Docked;
        _docked = placement.Docked;
        if (window.Left != placement.Left || window.Top != placement.Top)
            DesktopNative.MoveWithoutActivating(_windowHandle, placement.Left, placement.Top);
        if (changed) SyncNotchShape(); // Mulai dari bentuk berjalan; jangan meloncat pada batas snap.
    }

    private void FinishWindowDrag(bool resumeHover = true)
    {
        var moved = _repositioning;
        _dragArmed = _repositioning = false; // Bersihkan dulu sebelum LostMouseCapture dipanggil.
        if (IsMouseCaptured) ReleaseMouseCapture();
        if (moved)
        {
            Mouse.OverrideCursor = _cursorBeforeDrag; _cursorBeforeDrag = null;
            if (DesktopNative.GetWindowRect(_windowHandle, out var window))
            {
                _preferences.HasPosition = true; _preferences.DockedToTop = _docked;
                _preferences.CenterX = (window.Left + window.Right) / 2.0;
                _preferences.TopY = window.Top;
                SavePreferences(); // Hanya sekali setelah drag selesai.
            }
            if (Math.Abs(_dragDpi - VisualTreeHelper.GetDpi(this).DpiScaleX) > 0.001)
            { _measuredLyric = null; _coverFingerprint = null; QueueMetadata(); }
        }
        if (!resumeHover || _closed || _minimizedToTray) return;
        if (moved) SetExpanded(PointerOverNotch(), resume: true);
        else if (!IsMouseOver && !AnyMenuOpen) _collapseTimer.Start();
    }

    private bool PointerOverNotch()
    {
        if (!DesktopNative.TryGetCursor(out var point)
            || !DesktopNative.GetWindowRect(_windowHandle, out var window)) return false;
        var dpi = VisualTreeHelper.GetDpi(this);
        return NotchViewport.ContainsPoint(new Point((point.X - window.Left) / dpi.DpiScaleX,
            (point.Y - window.Top) / dpi.DpiScaleY));
    }

    private IntPtr Window_Hook(IntPtr hwnd, int message, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (_closed) return IntPtr.Zero;
        if (DesktopNative.RestoreMessage != 0 && (uint)message == DesktopNative.RestoreMessage)
        { handled = true; OnUi(RestoreFromTray); }
        else if (message == 0x031D) // WM_CLIPBOARDUPDATE
        { handled = true; QueueClipboardRead(); }
        else if (message == 0x0021 && (_gameLatched || DesktopNative.FullscreenAppOnMonitor(hwnd))) // WM_MOUSEACTIVATE
        { handled = true; return new IntPtr(3); } // MA_NOACTIVATE: game tidak kehilangan fokus/ter-minimize.
        return IntPtr.Zero;
    }

    private void SavePreferences()
    {
        if (!_preferences.Save()) StatusText.Text = "Pengaturan aktif, tetapi belum bisa disimpan.";
    }

    // Notifikasi WhatsApp/browser: Windows tidak mengirim event ke aplikasi tanpa paket (MSIX),
    // jadi daftar notifikasi dibaca tiap 2 detik. Satu panggilan WinRT ringan, tanpa jaringan.
    private readonly DispatcherTimer _noticePollTimer = new() { Interval = TimeSpan.FromSeconds(2) };
    private readonly DispatcherTimer _noticeTimer = new() { Interval = TimeSpan.FromSeconds(5) };
    private HashSet<uint>? _seenToasts; // null = daftar awal belum dibaca; notifikasi lama tidak ditampilkan.
    private string? _notice;
    private double _noticeWidth;
    private bool _noticePolling;

    private async Task StartNotificationsAsync()
    {
        try
        {
            var listener = Windows.UI.Notifications.Management.UserNotificationListener.Current;
            if (listener.GetAccessStatus() != Windows.UI.Notifications.Management.UserNotificationListenerAccessStatus.Allowed
                && await listener.RequestAccessAsync() != Windows.UI.Notifications.Management.UserNotificationListenerAccessStatus.Allowed)
                return; // Izin "akses notifikasi" dimatikan di Windows: fitur diam saja.
        }
        catch (Exception ex) { Debug.WriteLine(ex); return; }
        _noticeTimer.Tick += (_, _) => { _noticeTimer.Stop(); _notice = null; UpdateCompactState(); };
        _noticePollTimer.Tick += async (_, _) => await PollNotificationsAsync();
        await PollNotificationsAsync();
        _noticePollTimer.Start();
    }

    private async Task PollNotificationsAsync()
    {
        if (_noticePolling || _closed || _minimizedToTray) return;
        _noticePolling = true;
        try
        {
            var toasts = await Windows.UI.Notifications.Management.UserNotificationListener.Current
                .GetNotificationsAsync(Windows.UI.Notifications.NotificationKinds.Toast);
            var shown = toasts.Where(IsShown).ToList();
            var fresh = _seenToasts is null ? null
                : shown.Where(x => !_seenToasts.Contains(x.Id)).MaxBy(x => x.CreationTime);
            _seenToasts = shown.Select(x => x.Id).ToHashSet(); // Mengikuti isi Action Center, tidak tumbuh terus.
            if (fresh is not null && !_closed) ShowNotice(NoticeText(fresh));
        }
        catch (Exception ex) { Debug.WriteLine(ex); }
        finally { _noticePolling = false; }
    }

    private static List<string> Texts(Windows.UI.Notifications.UserNotification toast)
    {
        try
        {
            return toast.Notification?.Visual?.GetBinding(Windows.UI.Notifications.KnownNotificationBindings.ToastGeneric)?
                .GetTextElements().Select(x => (x.Text ?? "").Trim()).Where(x => x.Length > 0).ToList() ?? [];
        }
        catch (Exception) { return []; }
    }

    // WhatsApp Web di browser: Edge/Chrome tidak menyebut situs asal di notifikasinya,
    // jadi semua notifikasi browser ditampilkan. Aplikasi WhatsApp (bila dipasang) juga.
    private static readonly string[] ShownApps = ["WhatsApp", "Microsoft Edge", "Google Chrome", "Brave", "Opera", "Firefox"];

    private static bool IsShown(Windows.UI.Notifications.UserNotification toast)
    {
        try
        {
            var name = toast.AppInfo?.DisplayInfo?.DisplayName ?? "";
            return ShownApps.Any(x => name.Contains(x, StringComparison.OrdinalIgnoreCase));
        }
        catch (Exception) { return false; } // Beberapa aplikasi menolak membuka info tampilannya.
    }

    private static string NoticeText(Windows.UI.Notifications.UserNotification toast)
    {
        var texts = Texts(toast);
        if (texts.Count == 0) return "Notifikasi baru";
        var line = Regex.Replace(texts.Count > 1 ? $"{texts[0]}: {string.Join(" ", texts.Skip(1))}" : texts[0], @"\s+", " ");
        return line.Length > 80 ? line[..79] + "…" : line;
    }

    private void ShowNotice(string text)
    {
        _notice = text;
        CompactNotice.Text = text;
        var measured = new FormattedText(text, CultureInfo.CurrentUICulture, CompactNotice.FlowDirection,
            new Typeface(CompactNotice.FontFamily, CompactNotice.FontStyle, CompactNotice.FontWeight, CompactNotice.FontStretch),
            CompactNotice.FontSize, CompactNotice.Foreground, VisualTreeHelper.GetDpi(this).PixelsPerDip);
        _noticeWidth = Math.Ceiling(measured.WidthIncludingTrailingWhitespace);
        _noticeTimer.Stop(); _noticeTimer.Start();
        UpdateCompactState();
    }

    // Controller tersambung/terputus. Baterai sengaja tidak ditampilkan: pad lewat dongle (mis. Flydigi)
    // menyamar sebagai Xbox 360 kabel, dan Windows melaporkan baterai palsu yang selalu penuh.
    private long _padWatchStarted;

    private void StartPadWatch()
    {
        _padWatchStarted = Stopwatch.GetTimestamp();
        Windows.Gaming.Input.RawGameController.RawGameControllerAdded += (_, _) => PadChanged("Controller terhubung");
        Windows.Gaming.Input.RawGameController.RawGameControllerRemoved += (_, _) => PadChanged("Controller terputus");
        _ = Windows.Gaming.Input.RawGameController.RawGameControllers; // Akses pertama menyalakan pemantauan.
    }

    // Event datang dari thread lain. Controller yang sudah tersambung dilaporkan ulang saat aplikasi mulai.
    private void PadChanged(string text) => Dispatcher.BeginInvoke(() =>
    {
        if (!_closed && Stopwatch.GetElapsedTime(_padWatchStarted) > TimeSpan.FromSeconds(3)) ShowNotice(text);
    });

    private void UpdatePinButton()
    {
        PinSlash.Visibility = Topmost ? Visibility.Collapsed : Visibility.Visible;
        PinButton.Background = (Brush)FindResource(Topmost ? "NotchAccent" : "NotchSoft");
        PinButton.ToolTip = Topmost ? "Selalu di atas: aktif — klik untuk melepas pin" : "Selalu di atas: mati — klik untuk memasang pin";
        AutomationProperties.SetName(PinButton, Topmost ? "Nonaktifkan selalu di atas" : "Aktifkan selalu di atas");
    }

    private void PinButton_Click(object sender, RoutedEventArgs e)
    {
        Topmost = !Topmost; _preferences.PinOnTop = Topmost;
        if (_lyricsWindow is not null) _lyricsWindow.Topmost = Topmost;
        UpdatePinButton(); SavePreferences();
    }

    private void ResetPosition()
    {
        _preferences.HasPosition = false;
        _preferences.DockedToTop = _docked = true;
        SyncNotchShape();
        SavePreferences(); SnapNotchSize(); RefreshNotchSize();
    }

    // Background, Surface, Soft, Accent, Ink. Semua latar gelap karena teks memakai putih.
    private static readonly (string Name, string[] Colors)[] Themes =
    [
        ("Zaitun", ["#6A795D", "#8E9162", "#B2BD7E", "#D9D28B", "#182116"]),
        ("Malam", ["#1E2A44", "#34466B", "#7FA3D9", "#BFD7FF", "#0B1222"]),
        ("Grafit", ["#1C1C1E", "#3A3A3C", "#A1A1A6", "#E5E5EA", "#111113"]),
        ("Anggur", ["#4A2433", "#6E3A4D", "#C98AA0", "#F2C4CE", "#22101A"]),
        ("Biru Es", ["#112D4E", "#3F72AF", "#DBE2EF", "#F9F7F7", "#112D4E"]),
    ];

    // Tema kustom (bila sudah disimpan) menjadi urutan terakhir setelah tema bawaan.
    private int ThemeCount => Themes.Length + (_preferences.CustomColors is null ? 0 : 1);
    private string[] ThemeColors(int index)
        => index == Themes.Length && _preferences.CustomColors is { } custom ? custom : Themes[index % Themes.Length].Colors;

    private void ApplyTheme(int index)
    {
        index = ((index % ThemeCount) + ThemeCount) % ThemeCount;
        _preferences.Theme = index;
        ApplyColors(ThemeColors(index));
        ThemeButton.ToolTip = $"Tema: {(index == Themes.Length ? "Kustom" : Themes[index].Name)} — klik untuk ganti, klik kanan untuk edit";
    }

    internal void ApplyColors(string[] colors)
    {
        string[] keys = ["NotchBackground", "NotchSurface", "NotchSoft", "NotchAccent", "NotchInk"];
        for (var i = 0; i < keys.Length; i++)
        {
            var brush = new SolidColorBrush((Color)ColorConverter.ConvertFromString(colors[i]));
            brush.Freeze();
            Resources[keys[i]] = brush;
        }
        NotchViewport.SetPalette((Brush)Resources["NotchBackground"], (Brush)Resources["NotchSurface"]);
        // Warna ke-6 opsional: tema bawaan memakai warna aksen untuk garis progres.
        var progress = new SolidColorBrush((Color)ColorConverter.ConvertFromString(colors.Length > 5 ? colors[5] : colors[3]));
        progress.Freeze();
        NotchViewport.SetProgressBrush(progress);
        _lyricsWindow?.UseTheme(Resources);
        UpdatePinButton();
    }

    private void ThemeButton_Click(object sender, RoutedEventArgs e)
    {
        ApplyTheme(_preferences.Theme + 1);
        SavePreferences();
    }

    private ThemeEditor? _themeEditor;

    private void ThemeButton_RightClick(object sender, MouseButtonEventArgs e)
    {
        e.Handled = true; // Menu klik kanan notch tidak ikut terbuka.
        OpenThemeEditor();
    }

    private void OpenThemeEditor()
    {
        if (_themeEditor is not null) { _themeEditor.Activate(); return; }
        _themeEditor = new ThemeEditor(this, ThemeColors(_preferences.Theme));
        _themeEditor.Closed += (_, _) => _themeEditor = null;
        _themeEditor.Show();
    }

    internal void SaveCustomTheme(string[] colors)
    {
        _preferences.CustomColors = colors;
        ApplyTheme(Themes.Length); SavePreferences();
    }

    internal void CancelThemeEdit() => ApplyTheme(_preferences.Theme);

    private void MinimizeButton_Click(object sender, RoutedEventArgs e) => MinimizeToTray();

    private void MinimizeToTray()
    {
        if (_closed || _minimizedToTray) return;
        // Jangan menyembunyikan jendela tanpa jalur untuk memunculkannya kembali.
        if (_tray?.EnsureVisible() != true)
        { StatusText.Text = "Tray belum tersedia. Coba lagi setelah taskbar Windows siap."; return; }
        FinishWindowDrag(resumeHover: false);
        _minimizedToTray = true;
        _clipboardSearchTimer.Stop(); CompletePageTransition();
        _coverRefreshTimer.Stop();
        _metadataRetryTimer.Stop(); _skipFromTrackKey = null;
        _collapseTimer.Stop(); _clockTimer.Stop(); _progressTimer.Stop(); _lyricTimer.Stop();
        SetEqualizerAnimation(false); StopLyricScroll();
        _lyricsVersion++;
        if (_lyricsCts is not null) { _reloadLyricsAfterHide = true; _lyricsCts.Cancel(); }
        _metadataVersion++; _metadataPending = false;
        _lyricIndex = int.MinValue;
        // Simpan satu lagu aktif agar pilihan lirik dan koreksi waktunya tidak hilang.
        _lyricsWindow?.Close(); // Lepaskan kontrol, layout teks panjang, dan daftar hasil jendela Aa.
        Hide();
        _expanded = false; _transitionVersion++; _sizeVersion++;
        SetTransitionImmediately();
        CompactPanel.BeginAnimation(OpacityProperty, null); ExpandedHost.BeginAnimation(OpacityProperty, null);
        CompactPanel.Opacity = 1; CompactPanel.Visibility = Visibility.Visible; CompactPanel.IsHitTestVisible = true;
        ExpandedHost.Opacity = 0; ExpandedHost.Visibility = Visibility.Collapsed; ExpandedHost.IsHitTestVisible = false;
        _sizeTargetWidth = SmallWidth; _sizeTargetHeight = SmallHeight;
        SnapNotchSize();
        // Gambar dibaca kembali saat restore; pilihan lirik dan offset tetap dipertahankan.
        CoverImage.Source = null; _coverFingerprint = null;
    }

    private void RestoreFromTray()
    {
        if (_closed) return;
        var wasHidden = _minimizedToTray;
        _minimizedToTray = false;
        if (wasHidden) _awaitingMetadata = true;
        Show(); Activate();
        if (wasHidden) SetLyricMessage("Memuat ulang lirik…", "Lirik");
        UpdateClock(); UpdateCompactState(); PositionWindow();
        if (wasHidden) _ = InitializeMediaAsync();
    }

    private void SystemParameters_Changed(object? sender, PropertyChangedEventArgs e)
        => OnUi(() =>
        {
            if (_closed || _minimizedToTray) return;
            if (_repositioning) { _measuredLyric = null; return; }
            SnapNotchSize(); _measuredLyric = null; UpdateCompactState();
        });

    private void Window_MouseEnter(object sender, MouseEventArgs e)
    {
        _gameLatched = DesktopNative.FullscreenAppOnMonitor(_windowHandle);
        if (_repositioning || _dragArmed || _minimizedToTray || GameLocked) return;
        _collapseTimer.Stop();
        SetExpanded(true);
    }

    private void Window_MouseLeave(object sender, MouseEventArgs e)
    {
        if (!AnyMenuOpen && !_seekDragging && !_gameDragging && !_dragArmed && !_repositioning && !_minimizedToTray) _collapseTimer.Start();
    }

    private void CollapseTimer_Tick(object? sender, EventArgs e)
    {
        _collapseTimer.Stop();
        if (!IsMouseOver && !AnyMenuOpen && !_seekDragging && !_gameDragging && !_dragArmed && !_repositioning && !_minimizedToTray)
        { _gameUnlocked = false; SetExpanded(false); ScheduleTrim(); }
    }

    // Notch lebih sering diam: setelah aktivitas berat, bereskan sampah GC lalu kembalikan halaman
    // memori yang tidak dipakai ke Windows. Halaman yang dipakai lagi dimuat ulang dari memori standby (murah).
    private readonly DispatcherTimer _trimTimer = new() { Interval = TimeSpan.FromSeconds(4) };

    private void ScheduleTrim()
    {
        if (_trimTimer.Tag is null)
        {
            _trimTimer.Tag = true;
            _trimTimer.Tick += (_, _) =>
            {
                _trimTimer.Stop();
                if (_closed || _expanded) return; // Saat dipakai, biarkan memori tetap hangat.
                GC.Collect(2, GCCollectionMode.Forced, true, true);
                DesktopNative.TrimWorkingSet();
            };
        }
        _trimTimer.Stop(); _trimTimer.Start();
    }

    private void SetExpanded(bool expand, bool resume = false)
    {
        if ((_expanded == expand && !resume) || _closed || _minimizedToTray || _repositioning) return;
        StopLyricScroll();
        _expanded = expand;
        _hoverTransitioning = true;
        _hoverStartedTimestamp = Stopwatch.GetTimestamp();
        var version = ++_transitionVersion;
        var remaining = Math.Abs((expand ? 1 : 0) - ExpandedHost.Opacity);
        var fullDuration = LightAnimations ? 220 : 280;
        _hoverMilliseconds = (int)Math.Clamp(fullDuration * Math.Sqrt(remaining), 80, fullDuration);
        SyncNotchShape();
        // Media events sudah menjaga status pemutar; hover tidak perlu memanggil WinRT lagi.
        if (expand)
        {
            UpdateClock(); RenderProgress();
            if (_expandedPage == 1 && _clipboardResultsDirty) RefreshClipboardResults();
            if (_expandedPage == GamePageIndex) RefreshGames(); // "Terakhir dimainkan" bisa berubah sejak dibuka terakhir.
        }
        else
        {
            RenderProgress(); // Garis progres tepi muncul lagi saat notch menutup.
            _clipboardSearchTimer.Stop();
            if (ClipboardSearchBox.IsKeyboardFocusWithin) Keyboard.ClearFocus();
        }
        RenderLyrics(); // Mode kecil tetap mengikuti lagu, apa pun halaman terakhirnya.
        UpdateProgressTimer(); UpdateLyricTimer(); UpdateCompactState();
        // Siapkan teks, ukuran akhir, dan clip sebelum lapisan baru mulai terlihat.
        CompactPanel.Visibility = ExpandedHost.Visibility = Visibility.Visible;
        CompactPanel.IsHitTestVisible = ExpandedHost.IsHitTestVisible = false;
        FadePanel(CompactPanel, expand ? 0 : 1, version);
        FadePanel(ExpandedHost, expand ? 1 : 0, version);
        AnimateExpandedContent(expand, version);
    }

    private void FadePanel(FrameworkElement panel, double target, int version)
    {
        var from = panel.Opacity;
        panel.BeginAnimation(OpacityProperty, null);
        panel.Opacity = from;
        if (!SystemParameters.ClientAreaAnimation)
        {
            panel.Opacity = target; panel.Visibility = target == 0 ? Visibility.Collapsed : Visibility.Visible;
            if (target == 1) _hoverTransitioning = false;
            panel.IsHitTestVisible = target == 1; return;
        }
        // Kedua lapisan memakai awal, durasi, dan kurva yang sama.
        // Opacity saling melengkapi, termasuk ketika hover dibalik di tengah jalan.
        // Layout sudah disiapkan di SetExpanded, jadi tidak perlu fase kosong sebelum masuk.
        var animation = new DoubleAnimation(from, target, TimeSpan.FromMilliseconds(_hoverMilliseconds))
        {
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
            FillBehavior = FillBehavior.HoldEnd
        };
        Timeline.SetDesiredFrameRate(animation, AnimationFrames);
        animation.Completed += (_, _) =>
        {
            if (_closed || version != _transitionVersion) return;
            panel.Opacity = target;
            panel.BeginAnimation(OpacityProperty, null);
            if (target == 0) panel.Visibility = Visibility.Collapsed;
            else _hoverTransitioning = false;
            panel.IsHitTestVisible = target == 1;
        };
        panel.BeginAnimation(OpacityProperty, animation, HandoffBehavior.SnapshotAndReplace);
    }

    private void AnimateExpandedContent(bool entering, int version)
    {
        var from = ExpandedScale.ScaleX;
        var target = entering ? 1 : 0.94;
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
        ExpandedScale.ScaleX = ExpandedScale.ScaleY = target;
        if (!SystemParameters.ClientAreaAnimation) return;
        // Mulai dari nilai yang sedang tampil, termasuk saat animasi berbalik arah.
        var animation = new DoubleAnimation(from, target, TimeSpan.FromMilliseconds(_hoverMilliseconds))
        {
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
            FillBehavior = FillBehavior.Stop
        };
        Timeline.SetDesiredFrameRate(animation, AnimationFrames);
        animation.Completed += (_, _) =>
        {
            if (!_closed && version == _transitionVersion)
            {
                ExpandedScale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
                ExpandedScale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
            }
        };
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleXProperty, animation, HandoffBehavior.SnapshotAndReplace);
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleYProperty, animation, HandoffBehavior.SnapshotAndReplace);
    }

    private void SetTransitionImmediately()
    {
        _hoverTransitioning = false;
        ++_transitionVersion;
        CompactPanel.BeginAnimation(OpacityProperty, null); ExpandedHost.BeginAnimation(OpacityProperty, null);
        CompactPanel.Opacity = _expanded ? 0 : 1; ExpandedHost.Opacity = _expanded ? 1 : 0;
        CompactPanel.Visibility = _expanded ? Visibility.Collapsed : Visibility.Visible;
        ExpandedHost.Visibility = _expanded ? Visibility.Visible : Visibility.Collapsed;
        CompactPanel.IsHitTestVisible = !_expanded; ExpandedHost.IsHitTestVisible = _expanded;
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
        ExpandedScale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
        ExpandedScale.ScaleX = ExpandedScale.ScaleY = _expanded ? 1 : 0.94;
        SyncNotchShape(false);
    }

    private void SyncNotchShape(bool animate = true)
    {
        NotchViewport.SetDockCut(_docked,
            animate && SystemParameters.ClientAreaAnimation && !_closed && !_minimizedToTray,
            _repositioning ? (LightAnimations ? 140 : 180) : _hoverMilliseconds, AnimationFrames);
    }

    private void SourceButton_Click(object sender, RoutedEventArgs e)
    {
        NotchMenu.PlacementTarget = SourceButton;
        NotchMenu.Placement = PlacementMode.Bottom;
        NotchMenu.IsOpen = true;
    }

    private void ControlsMenu_Opened(object sender, RoutedEventArgs e) => _collapseTimer.Stop();

    private void ControlsMenu_Closed(object sender, RoutedEventArgs e)
    {
        if (!_closed && !_minimizedToTray && !_repositioning && !IsMouseOver && !_seekDragging) _collapseTimer.Start();
    }

    private void LyricsButton_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrWhiteSpace(_songTitle)) return;
        if (_lyricsWindow is null)
        {
            _lyricsWindow = new LyricsWindow(this) { Owner = this };
            _lyricsWindow.Closed += (_, _) => _lyricsWindow = null;
        }
        UpdateLyricsWindow();
        _lyricsWindow.Show();
        _lyricsWindow.Activate();
    }

    private void UpdateLyricsWindow()
        => _lyricsWindow?.ShowState(_songTitle, _songArtist, _lyricsMode, _lyricsResult, _lyricChoice,
            _lyricDocument, (_lyricsStatus + " " + _cacheStatus).Trim());

    private void EnsureLyricsTrack()
    {
        if (_minimizedToTray) return;
        var key = $"{_songTitle}\u001F{_songArtist}\u001F{_songAlbum}";
        var changed = _lyricsTrackKey != key;
        if (!changed && !_reloadLyricsAfterHide) return;
        _lyricsTrackKey = key;
        _reloadLyricsAfterHide = false;
        if (changed)
        {
            _lyricsOffsetSeconds = 0; // Minimize tidak mereset koreksi lagu yang sama.
            SetLyricsMode(LyricsMode.Auto); // Setiap lagu baru mendukung semua bahasa secara bawaan.
        }
        if (string.IsNullOrWhiteSpace(_songTitle))
        {
            _lyricsVersion++; _lyricsCts?.Cancel(); _lyricsResult = null; _lyricChoice = null; _lyricDocument = null;
            _lyricTimer.Stop(); _lyricsStatus = "Belum ada lagu."; _cacheStatus = "";
            SetLyricMessage("Belum ada lagu", "Putar musik untuk menampilkan lirik"); UpdateLyricsWindow();
            return;
        }
        _ = LoadLyricsAsync(changed ? _songTitle : _lastLyricsTitle,
            changed ? _songArtist : _lastLyricsArtist, _lyricsMode);
    }

    internal void SearchLyrics(string title, string artist, LyricsMode mode)
    {
        if (_closed || _minimizedToTray || string.IsNullOrWhiteSpace(_songTitle)) return;
        SetLyricsMode(mode);
        _ = LoadLyricsAsync(title, artist, mode, forceNetwork: true);
    }

    private void SetLyricsMode(LyricsMode mode) => _lyricsMode = mode;

    internal void SetLyricsOffset(double seconds)
    {
        if (_closed || !double.IsFinite(seconds)) return;
        _lyricsOffsetSeconds = Math.Clamp(seconds, -120, 120);
        RenderLyrics(); UpdateLyricsWindow();
        _ = SaveActiveLyricsAsync();
    }

    private async Task LoadLyricsAsync(string title, string artist, LyricsMode mode, bool forceNetwork = false)
    {
        _lastLyricsTitle = title; _lastLyricsArtist = artist;
        _lyricsCts?.Cancel();
        var version = ++_lyricsVersion;
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(75));
        _lyricsCts = cts;
        var token = cts.Token;
        _cacheIdentity = LyricCache.Identity(_songTitle, _songArtist, _songAlbum);
        _cacheDuration = _hasTimeline ? Math.Max(0, _endSeconds - _startSeconds) : 0;
        _cacheStatus = "";
        _lyricsResult = null; _lyricChoice = null; _lyricDocument = null; _lyricIndex = int.MinValue;
        _lyricTimer.Stop();
        _lyricsStatus = "Mencari lirik…";
        SetLyricMessage("Mencari lirik…", "Lirik"); UpdateLyricsWindow();
        var progress = new Progress<string>(message =>
        {
            if (!_closed && _lyricDocument is null && version == _lyricsVersion && ReferenceEquals(_lyricsCts, cts) && !token.IsCancellationRequested)
            { _lyricsStatus = message; UpdateLyricsWindow(); }
        });
        try
        {
            if (string.IsNullOrWhiteSpace(title)) throw new LrcServiceException("Isi judul lagu lalu Cari ulang.");
            await Task.Delay(450, token); // Gabungkan perubahan metadata yang datang beruntun.
            if (_closed || version != _lyricsVersion) return;
            RefreshTimeline(); // Durasi sering datang sesaat setelah judul ketika berpindah pemutar.
            _cacheDuration = _hasTimeline ? Math.Max(0, _endSeconds - _startSeconds) : 0;
            if (!forceNetwork)
            {
                await _cacheWriteTask.WaitAsync(token);
                var identity = _cacheIdentity;
                var cachedDuration = _cacheDuration;
                var saved = await Task.Run(() => LyricCache.ReadAsync(identity, mode, cachedDuration, token), token);
                token.ThrowIfCancellationRequested();
                if (_closed || version != _lyricsVersion) return;
                if (saved is not null)
                {
                    var stored = saved.ToRecord();
                    var cachedChoice = new LyricCandidate(stored, 1, true);
                    _lyricsResult = new([cachedChoice], LyricsLanguage.PrefersRomaji(mode, stored), "");
                    _lyricsOffsetSeconds = saved.OffsetSeconds;
                    var linked = saved.Identity != identity;
                    _cacheStatus = linked ? "Lagu dikenali lintas sumber; memakai lirik tersimpan."
                        : "Dibaca dari penyimpanan lokal, tanpa pencarian internet.";
                    ApplyLyrics(cachedChoice, LyricDocument.From(stored));
                    if (linked || saved.Mode != mode) await SaveActiveLyricsAsync(linked
                        ? "Lagu dikenali lintas sumber. Lirik tersimpan dipakai dan pilihan ini diingat otomatis."
                        : "Lirik dari simpanan lama dipakai dan diingat untuk mode Otomatis, tanpa mencari ulang.");
                    return;
                }
            }
            var sourceQuery = SongMatcher.CleanQuery(_songTitle, _songArtist);
            var searchQuery = SongMatcher.CleanQuery(title, artist);
            var same = string.Equals(sourceQuery.Title, searchQuery.Title, StringComparison.OrdinalIgnoreCase)
                && string.Equals(sourceQuery.Artist, searchQuery.Artist, StringComparison.OrdinalIgnoreCase);
            var album = same ? _songAlbum : "";
            var duration = same && _hasTimeline ? _endSeconds - _startSeconds : 0;
            // Parsing JSON dan fuzzy tidak menahan thread animasi/antarmuka.
            LyricsSearchResult result;
            await _lyricSearchGate.WaitAsync(token);
            try
            {
                result = await Task.Run(() => LyricsSearch.FindAsync(title, artist, album,
                    duration, mode, token, progress), token);
            }
            finally { _lyricSearchGate.Release(); }
            token.ThrowIfCancellationRequested();
            if (_closed || version != _lyricsVersion) return;
            _lyricsResult = result;
            var choice = SongMatcher.AutoChoice(result.Choices);
            if (choice is not null) await PrepareChoiceAsync(choice, version, token, progress);
            else
            {
                var hint = result.Choices.Count > 0 ? "Pilih hasil lewat Aa" : "Lirik belum tersedia";
                _lyricsStatus = (hint + ". " + result.Note).Trim();
                SetLyricMessage(hint, "Aa untuk pencarian"); UpdateLyricsWindow();
            }
        }
        catch (OperationCanceledException)
        {
            if (!_closed && version == _lyricsVersion)
            { _lyricsStatus = "Pencarian terlalu lama. Buka Aa untuk mencoba lagi atau Muat file lirik."; SetLyricMessage("Lirik belum dimuat", "Coba lagi lewat Aa"); UpdateLyricsWindow(); }
        }
        catch (Exception ex)
        {
            if (!_closed && version == _lyricsVersion)
            { _lyricsStatus = ex is LrcServiceException ? ex.Message : "Lirik belum bisa dimuat. Periksa koneksi lalu buka Aa."; SetLyricMessage("Lirik belum dimuat", "Coba lagi lewat Aa"); UpdateLyricsWindow(); }
        }
        finally { if (ReferenceEquals(_lyricsCts, cts)) _lyricsCts = null; }
    }

    internal async Task ShowSavedLyricsAsync(string title, string artist, LyricsMode mode)
    {
        if (_closed || _minimizedToTray || !CanImportLyrics) return;
        _lyricsCts?.Cancel();
        var version = ++_lyricsVersion;
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        _lyricsCts = cts;
        var token = cts.Token;
        SetLyricsMode(mode);
        _cacheIdentity = LyricCache.Identity(_songTitle, _songArtist, _songAlbum);
        RefreshTimeline();
        _cacheDuration = _hasTimeline ? Math.Max(0, _endSeconds - _startSeconds) : 0;
        _lyricsResult = null; _lyricChoice = null; _lyricDocument = null; _lyricIndex = int.MinValue;
        _lyricTimer.Stop(); _cacheStatus = "";
        _lyricsStatus = "Membaca daftar lirik tersimpan…";
        SetLyricMessage("Pilih lirik tersimpan", "Pilih hasil melalui Aa"); UpdateLyricsWindow();
        try
        {
            await _cacheWriteTask.WaitAsync(token);
            var duration = _cacheDuration;
            var choices = await Task.Run(() => LyricCache.ListChoicesAsync(title, artist, mode, duration, token), token);
            token.ThrowIfCancellationRequested();
            if (_closed || version != _lyricsVersion) return;
            _lyricsResult = new(choices, mode == LyricsMode.Romaji,
                "Pilihan lirik tersimpan dihubungkan dengan lagu aktif dan diingat otomatis.");
            _lyricsStatus = choices.Count > 0
                ? "Pilih rekaman yang sama pada daftar Hasil lirik. Jika belum terlihat, perbaiki kolom Judul / Artis lalu klik Lirik tersimpan lagi."
                : "Belum ada lirik tersimpan yang bisa dibaca. Gunakan Cari ulang atau Muat file .lrc.";
            UpdateLyricsWindow();
        }
        catch (OperationCanceledException)
        { if (!_closed && version == _lyricsVersion) { _lyricsStatus = "Pembacaan dibatalkan atau terlalu lama. Klik Lirik tersimpan untuk mencoba lagi."; UpdateLyricsWindow(); } }
        catch (Exception)
        { if (!_closed && version == _lyricsVersion) { _lyricsStatus = "Daftar lirik tersimpan belum bisa dibaca."; UpdateLyricsWindow(); } }
        finally { if (ReferenceEquals(_lyricsCts, cts)) _lyricsCts = null; }
    }

    internal void ChooseLyrics(LyricCandidate choice)
    {
        if (_closed || _lyricsResult is null || !_lyricsResult.Choices.Contains(choice)) return;
        _ = ChooseLyricsAsync(choice);
    }

    private async Task ChooseLyricsAsync(LyricCandidate choice)
    {
        _lyricsCts?.Cancel();
        var version = ++_lyricsVersion;
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(35));
        _lyricsCts = cts;
        try { await PrepareChoiceAsync(choice, version, cts.Token, null); }
        catch (OperationCanceledException)
        {
            if (!_closed && version == _lyricsVersion)
            { _lyricsStatus = "Pemuatan lirik melewati batas waktu. Coba lagi atau Muat file .lrc."; SetLyricMessage("Lirik belum dimuat", "Coba lagi lewat Aa"); UpdateLyricsWindow(); }
        }
        catch (Exception ex)
        {
            if (!_closed && version == _lyricsVersion)
            { _lyricsStatus = ex is LrcServiceException ? ex.Message : "Lirik belum dapat dipakai. Coba hasil lain."; SetLyricMessage("Lirik belum dimuat", "Coba lagi lewat Aa"); UpdateLyricsWindow(); }
        }
        finally { if (ReferenceEquals(_lyricsCts, cts)) _lyricsCts = null; }
    }

    private async Task PrepareChoiceAsync(LyricCandidate choice, int version, CancellationToken token, IProgress<string>? progress)
    {
        if (_lyricsResult is null) return;
        _lyricChoice = null; _lyricDocument = null; _cacheStatus = "";
        _lyricTimer.Stop(); SetLyricMessage("Menyiapkan lirik…", "Lirik"); UpdateLyricsWindow();
        var chosenFromCache = choice.Record.CacheFileName is not null;
        if (choice.Record.CacheFileName is string fileName)
        {
            var saved = await Task.Run(() => LyricCache.ReadChosenAsync(fileName, token), token);
            token.ThrowIfCancellationRequested();
            if (_closed || version != _lyricsVersion) return;
            if (saved is null) throw new LrcServiceException("File lirik tersimpan tidak bisa dibaca. Pilih hasil lain atau cari ulang.");
            var placeholder = choice;
            choice = choice with { Record = saved.ToRecord() };
            _lyricsOffsetSeconds = saved.OffsetSeconds;
            _lyricsResult = _lyricsResult with { Choices = LyricLimits.Retain(new[] { choice }
                .Concat(_lyricsResult.Choices.Where(x => !ReferenceEquals(x, placeholder))).ToList()) };
        }
        var document = await Task.Run(() => LyricDocument.From(choice.Record), token);
        token.ThrowIfCancellationRequested();
        if (_closed || version != _lyricsVersion) return;
        // Tentukan dari hasil yang dipilih, bukan dari mode/hasil lagu sebelumnya.
        _lyricsResult = _lyricsResult with { WantRomaji = LyricsLanguage.PrefersRomaji(_lyricsMode, choice.Record) };
        if (_lyricsResult.WantRomaji && !document.Instrumental
            && UnisonClient.HasJapanese(document))
        {
            string fallbackNote = "";
            if (!document.IsSynced)
                fallbackNote = "Lirik asli tersedia tanpa penanda waktu. Romaji berwaktu belum tersedia.";
            else
            {
                _lyricsStatus = "Mengubah baris Jepang menjadi romaji (lokal, tanpa internet)…"; UpdateLyricsWindow();
                progress?.Report(_lyricsStatus);
                using var conversion = CancellationTokenSource.CreateLinkedTokenSource(token);
                conversion.CancelAfter(TimeSpan.FromSeconds(25));
                try
                {
                    var originalChoice = choice;
                    LrcRecord converted;
                    try
                    {
                        // Utama: penganalisis Jepang bawaan Windows. Cepat, offline, dan waktu baris tetap sama.
                        var source = choice.Record;
                        converted = await Task.Run(() => LocalRomaji.Romanize(source, document, conversion.Token), conversion.Token);
                    }
                    catch (Exception ex) when (ex is not OperationCanceledException)
                    {
                        // Cadangan: layanan Unison bila Windows tidak bisa membaca sebagian baris.
                        Debug.WriteLine(ex);
                        _lyricsStatus = "Romaji lokal gagal; mencoba Unison…"; UpdateLyricsWindow();
                        progress?.Report(_lyricsStatus);
                        converted = await Task.Run(() => UnisonClient.RomanizeAsync(choice.Record, document, conversion.Token), conversion.Token);
                    }
                    token.ThrowIfCancellationRequested();
                    if (_closed || version != _lyricsVersion) return;
                    choice = choice with { Record = converted };
                    _lyricsResult = _lyricsResult with { Choices = LyricLimits.Retain(new[] { choice }
                        .Concat(_lyricsResult.Choices.Where(x => !ReferenceEquals(x, originalChoice))).ToList()) };
                    document = LyricDocument.From(converted);
                }
                catch (OperationCanceledException) when (!token.IsCancellationRequested)
                { fallbackNote = "Konversi romaji terlalu lama; lirik asli beserta waktunya tetap digunakan."; }
                catch (Exception ex) when (ex is LrcServiceException or HttpRequestException or IOException or JsonException)
                { fallbackNote = "Romaji belum berhasil dibuat; lirik asli beserta waktunya tetap digunakan."; }
            }
            token.ThrowIfCancellationRequested();
            if (_closed || version != _lyricsVersion) return;
            if (fallbackNote.Length > 0) _lyricsResult = _lyricsResult with { Note = (fallbackNote + " " + _lyricsResult.Note).Trim() };
        }
        if (document.Preview.Length == 0 && !document.Instrumental)
            throw new LrcServiceException("Teks kosong, format belum didukung, atau melewati batas ukuran. Pilih hasil lain.");
        token.ThrowIfCancellationRequested();
        if (_closed || version != _lyricsVersion) return;
        ApplyLyrics(choice, document);
        await SaveActiveLyricsAsync(chosenFromCache ? "Pilihan lirik tersimpan sudah diingat untuk lagu aktif. Tidak perlu memilihnya lagi saat metadata yang sama diputar." : null);
    }

    private void ApplyLyrics(LyricCandidate choice, LyricDocument document)
    {
        _lyricChoice = choice; _lyricDocument = document; _lyricIndex = int.MinValue;
        ScheduleTrim(); // Pencarian lirik meninggalkan banyak objek sementara (JSON, HTTP, romaji).
        _lyricsStatus = choice.Record.Source + ": " + (_lyricDocument.Instrumental ? "Lagu instrumental."
            : _lyricDocument.Preview.Length == 0 ? "Teks kosong, format belum didukung, atau melewati batas ukuran. Pilih hasil lain."
            : _lyricDocument.IsSynced ? "Lirik berwaktu aktif; baris mengikuti posisi lagu."
            : "Teks tersedia tanpa penanda waktu. Tidak ada auto-scroll mengikuti lagu.")
            + (choice.Record.MachineRomanized ? " Romaji hasil konversi; bacaan kanji mungkin perlu diperbaiki." : "")
            + " " + (_lyricsResult?.Note ?? "");
        RenderLyrics(); UpdateLyricTimer(); UpdateLyricsWindow();
    }

    private async Task SaveActiveLyricsAsync(string? successMessage = null)
    {
        var choice = _lyricChoice;
        var document = _lyricDocument;
        if (choice is null || document is null || _cacheIdentity.Length == 0
            || (document.Preview.Length == 0 && !document.Instrumental)) return;
        var version = _lyricsVersion;
        var offset = _lyricsOffsetSeconds;
        if (_hasTimeline) _cacheDuration = Math.Max(0, _endSeconds - _startSeconds);
        var entry = CachedLyrics.Create(_cacheIdentity, _lyricsMode, _cacheDuration, offset, choice.Record, document);
        // Urutan ditetapkan di thread UI: klik koreksi baru tidak boleh ditimpa penulisan lama.
        var write = _cacheWriteTask.ContinueWith(_ => LyricCache.WriteAsync(entry), CancellationToken.None,
            TaskContinuationOptions.None, TaskScheduler.Default).Unwrap();
        _cacheWriteTask = write;
        var saved = await write;
        if (_closed || version != _lyricsVersion || !ReferenceEquals(choice, _lyricChoice) || offset != _lyricsOffsetSeconds) return;
        _cacheStatus = saved ? (successMessage ?? "Tersimpan otomatis untuk lagu ini. Bisa dipakai lagi tanpa mencari.")
            : "Lirik aktif, tetapi penyimpanan lokal gagal. Gunakan Simpan .lrc / .txt untuk membuat salinan.";
        UpdateLyricsWindow();
    }

    internal bool CanExportLyrics => !_awaitingMetadata && !_fileBusy && _lyricDocument is { Instrumental: false } doc && doc.Preview.Length > 0;
    internal bool CanImportLyrics => !_awaitingMetadata && !_fileBusy && !string.IsNullOrWhiteSpace(_songTitle);

    internal async Task ExportLyricsAsync()
    {
        if (!CanExportLyrics || _lyricChoice is null || _lyricDocument is null) return;
        var document = _lyricDocument;
        var record = _lyricChoice.Record;
        var offset = _lyricsOffsetSeconds;
        _fileBusy = true; UpdateLyricsWindow();
        try
        {
            var dialog = new Microsoft.Win32.SaveFileDialog { Title = "Simpan lirik", AddExtension = true,
                OverwritePrompt = true, DefaultExt = document.IsSynced ? ".lrc" : ".txt",
                Filter = document.IsSynced ? "Lirik berwaktu (*.lrc)|*.lrc" : "Teks lirik (*.txt)|*.txt",
                FileName = LyricFiles.FileName(record) + (document.IsSynced ? ".lrc" : ".txt") };
            if (dialog.ShowDialog(_lyricsWindow ?? (Window)this) != true) return;
            var content = LyricFiles.Export(record, document, offset);
            await Task.Run(() => LyricFiles.WriteAtomicAsync(dialog.FileName, content));
            if (!_closed && ReferenceEquals(document, _lyricDocument))
                _cacheStatus = "File lirik berhasil disimpan. Gunakan Muat file untuk membukanya kembali.";
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException or LrcServiceException)
        { if (!_closed) _cacheStatus = ex is LrcServiceException ? ex.Message : "File belum bisa disimpan. Pilih folder lain."; }
        finally { _fileBusy = false; UpdateLyricsWindow(); }
    }

    internal async Task ImportLyricsAsync(LyricsMode mode)
    {
        if (!CanImportLyrics) return;
        var before = _lyricsVersion;
        _fileBusy = true; UpdateLyricsWindow();
        try
        {
            var dialog = new Microsoft.Win32.OpenFileDialog { Title = "Muat lirik untuk lagu yang sedang diputar",
                Filter = "Lirik (*.lrc;*.txt)|*.lrc;*.txt", Multiselect = false, CheckFileExists = true };
            if (dialog.ShowDialog(_lyricsWindow ?? (Window)this) != true) return;
            if (_closed || before != _lyricsVersion)
            { _cacheStatus = "Lagu atau pencarian telah berubah. Pilih file lagi untuk lagu saat ini."; return; }
            _lyricsCts?.Cancel();
            var version = ++_lyricsVersion;
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(35));
            _lyricsCts = cts;
            try
            {
                SetLyricsMode(mode);
                var query = SongMatcher.CleanQuery(_songTitle, _songArtist);
                _cacheIdentity = LyricCache.Identity(_songTitle, _songArtist, _songAlbum);
                _cacheDuration = _hasTimeline ? Math.Max(0, _endSeconds - _startSeconds) : 0;
                var record = await Task.Run(() => LyricFiles.ReadAsync(dialog.FileName, query.Title, query.Artist, mode, cts.Token), cts.Token);
                cts.Token.ThrowIfCancellationRequested();
                if (_closed || version != _lyricsVersion) return;
                var choice = new LyricCandidate(record, 1, true);
                _lyricsResult = new([choice], mode == LyricsMode.Romaji || (mode == LyricsMode.Auto
                    && (record.IsRomanized || UnisonClient.IsJapanese(record))), "File dipilih untuk lagu aktif.");
                _lyricsOffsetSeconds = record.ImportedOffsetSeconds ?? 0;
                await PrepareChoiceAsync(choice, version, cts.Token, null);
            }
            catch (OperationCanceledException)
            { if (!_closed && version == _lyricsVersion) _cacheStatus = "Pemuatan dibatalkan atau terlalu lama."; }
            catch (Exception ex)
            { if (!_closed && version == _lyricsVersion) _cacheStatus = ex is LrcServiceException ? ex.Message : "File lirik belum bisa dibaca."; }
            finally { if (ReferenceEquals(_lyricsCts, cts)) _lyricsCts = null; }
        }
        finally { _fileBusy = false; UpdateLyricsWindow(); }
    }

    private void SetLyricMessage(string text, string hint)
    {
        StopLyricScroll();
        CompactLyric.Text = CurrentLyric.Text = text;
        PreviousLyric.Text = NextLyric.Text = ""; InlineLyricsHint.Text = hint;
        CompactLyric.ToolTip = text; CurrentLyric.ToolTip = text;
        UpdateCompactState();
    }

    private void UpdateLyricTimer()
    {
        var position = EstimatedPosition();
        var moving = _playbackRate > 0 ? position < _endSeconds : _playbackRate < 0 && position > _startSeconds;
        if (!_closed && !_minimizedToTray && (!_expanded || _expandedPage == 0 || _lyricsWindow?.IsVisible == true)
            && !_awaitingMetadata && _playing && _hasTimeline && _lyricDocument?.IsSynced == true && moving) _lyricTimer.Start();
        else _lyricTimer.Stop();
    }

    private void LyricTimer_Tick(object? sender, EventArgs e) { RenderLyrics(); UpdateLyricTimer(); }

    private void RenderLyrics()
    {
        var doc = _lyricDocument;
        if (_closed || _minimizedToTray || _awaitingMetadata || doc is null) return;
        int index;
        string previous = "", current, next = "";
        if (doc.Instrumental) { index = -4; current = "Instrumental"; InlineLyricsHint.Text = "Lagu tanpa lirik"; }
        else if (!doc.IsSynced)
        {
            index = -3;
            var lines = doc.Preview;
            current = lines.FirstOrDefault() ?? "Teks lirik belum tersedia"; next = lines.ElementAtOrDefault(1) ?? "";
            InlineLyricsHint.Text = "Tanpa penanda waktu · baca lewat Aa";
        }
        else if (!_hasTimeline) { index = -2; current = doc.Lines[0].Text; InlineLyricsHint.Text = "Pemutar belum mengirim waktu lagu"; }
        else
        {
            // Nilai positif menunda lirik. Posisi negatif tetap berarti intro.
            index = doc.IndexAt(EstimatedPosition() - _startSeconds - _lyricsOffsetSeconds);
            current = index < 0 ? "Intro…" : string.IsNullOrWhiteSpace(doc.Lines[index].Text) ? "♪" : doc.Lines[index].Text;
            previous = index > 0 ? doc.Lines[index - 1].Text : "";
            next = index + 1 < doc.Lines.Length ? doc.Lines[index + 1].Text : "";
            InlineLyricsHint.Text = _lyricChoice is not null && LrcClient.IsMarkedRomaji(_lyricChoice.Record)
                ? (_playing ? "Romaji · sinkron" : "Romaji · dijeda")
                : (_playing ? "Lirik · sinkron" : "Lirik · dijeda");
        }
        if (index == _lyricIndex) return;
        var oldText = CurrentLyric.Text;
        var canScroll = _lyricIndex >= -1 && index >= 0 && _playing && !_repositioning && !_hoverTransitioning
            && (!_expanded || _expandedPage == 0)
            && SystemParameters.ClientAreaAnimation;
        _lyricIndex = index;
        PreviousLyric.Text = previous; CurrentLyric.Text = current; NextLyric.Text = next;
        CompactLyric.Text = current; CompactLyric.ToolTip = CurrentLyric.ToolTip = current;
        UpdateCompactState();
        StopLyricScroll();
        if (canScroll && oldText != current)
            ScrollLyricDown(oldText);
    }

    private void StopLyricScroll()
    {
        _lyricAnimationVersion++;
        foreach (var shift in _lyricShifts)
        { shift.BeginAnimation(TranslateTransform.YProperty, null); shift.Y = 0; }
        CompactOutgoing.Visibility = ExpandedOutgoing.Visibility = Visibility.Collapsed;
        CompactOutgoingLyric.Text = ExpandedOutgoingLyric.Text = "";
    }

    private void ScrollLyricDown(string oldText)
    {
        var version = _lyricAnimationVersion;
        var incoming = _expanded ? ExpandedIncomingShift : CompactIncomingShift;
        var outgoing = _expanded ? ExpandedOutgoingShift : CompactOutgoingShift;
        var layer = _expanded ? ExpandedOutgoing : CompactOutgoing;
        var oldLine = _expanded ? ExpandedOutgoingLyric : CompactOutgoingLyric;
        var viewport = _expanded ? ExpandedLyricViewport : CompactLyricViewport;
        var distance = Math.Max(26, viewport.ActualHeight);
        oldLine.Text = oldText; layer.Visibility = Visibility.Visible;
        // Arah turun: baris lama keluar ke bawah, baris baru datang dari atas.
        // Hanya dua lapisan teks untuk mode yang terlihat; tidak ada timer animasi permanen.
        // Gerakkan pembungkus berukuran viewport, bukan isi Viewbox yang diskalakan.
        // Tetapkan posisi awal sebelum frame baru agar teks tidak sempat tampil diam.
        incoming.Y = -distance; outgoing.Y = 0;
        var enter = new DoubleAnimation(-distance, 0, TimeSpan.FromMilliseconds(LyricScrollMilliseconds))
        { EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut }, FillBehavior = FillBehavior.HoldEnd };
        var leave = new DoubleAnimation(0, distance, TimeSpan.FromMilliseconds(LyricScrollMilliseconds))
        { EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut }, FillBehavior = FillBehavior.HoldEnd };
        Timeline.SetDesiredFrameRate(enter, AnimationFrames); Timeline.SetDesiredFrameRate(leave, AnimationFrames);
        enter.Completed += (_, _) => { if (!_closed && version == _lyricAnimationVersion) StopLyricScroll(); };
        outgoing.BeginAnimation(TranslateTransform.YProperty, leave, HandoffBehavior.SnapshotAndReplace);
        incoming.BeginAnimation(TranslateTransform.YProperty, enter, HandoffBehavior.SnapshotAndReplace);
    }

    private void RefreshNotchSize()
    {
        if (_closed || _minimizedToTray || _repositioning) return;
        // Ukur hanya saat teks/DPI berubah, bukan pada setiap tick atau frame animasi.
        if (_measuredLyric != CompactLyric.Text)
        {
            _measuredLyric = CompactLyric.Text;
            var text = new FormattedText(_measuredLyric, CultureInfo.CurrentUICulture,
                CompactLyric.FlowDirection, new Typeface(CompactLyric.FontFamily,
                    CompactLyric.FontStyle, CompactLyric.FontWeight, CompactLyric.FontStretch),
                CompactLyric.FontSize, CompactLyric.Foreground, VisualTreeHelper.GetDpi(this).PixelsPerDip);
            _compactTextWidth = Math.Ceiling(text.WidthIncludingTrailingWhitespace);
        }
        var screenWidth = Math.Max(1, _windowHandle == IntPtr.Zero ? SystemParameters.WorkArea.Width - 20
            : ActiveScreenArea().Width / VisualTreeHelper.GetDpi(this).DpiScaleX - 20);
        var music = HasCompactLyrics;
        var notice = _notice is not null;
        var wantedWidth = _expanded ? ExpandedWidth
            : notice ? Math.Max(MusicMinWidth, _noticeWidth + CompactInset * 2 + 6)
            : music ? Math.Max(MusicMinWidth, _compactTextWidth + CompactChrome) : SmallWidth;
        var width = Math.Min(screenWidth, wantedWidth);
        var height = _expanded ? LargeHeight : music || notice ? MusicHeight : SmallHeight;
        if (Math.Abs(width - _sizeTargetWidth) < 0.5 && height == _sizeTargetHeight) return;
        var milliseconds = _hoverTransitioning
            ? (int)Math.Max(1, _hoverMilliseconds - Stopwatch.GetElapsedTime(_hoverStartedTimestamp).TotalMilliseconds)
            : height == _sizeTargetHeight ? (LightAnimations ? 180 : 240) : _hoverMilliseconds;
        _sizeTargetWidth = width; _sizeTargetHeight = height;
        MorphNotch(milliseconds);
    }

    private Rect TargetScreenBounds(double width, double height)
    {
        var dpi = VisualTreeHelper.GetDpi(this);
        var area = ActiveScreenArea();
        var pixelWidth = Math.Ceiling(width * dpi.DpiScaleX);
        var pixelHeight = Math.Ceiling(height * dpi.DpiScaleY);
        var center = _preferences.HasPosition ? _preferences.CenterX : (area.Left + area.Right) / 2.0;
        var left = Math.Clamp(center - pixelWidth / 2, area.Left, Math.Max(area.Left, area.Right - pixelWidth));
        var top = _docked ? area.Top : Math.Clamp(_preferences.TopY, area.Top, Math.Max(area.Top, area.Bottom - pixelHeight));
        return new Rect(Math.Round(left), Math.Round(top), pixelWidth, pixelHeight);
    }

    // Panggung jendela tetap: selalu memuat ukuran terbuka, sehingga hover besar <-> kecil
    // tidak perlu mengubah ukuran/posisi HWND. Area di luar notch transparan (alpha 0),
    // jadi klik tetap tembus ke aplikasi di belakangnya.
    private Rect StageScreenBounds(Rect targetScreen)
    {
        var screenWidth = ActiveScreenArea().Width / VisualTreeHelper.GetDpi(this).DpiScaleX - 20;
        var expandedWidth = Math.Max(1, Math.Min(ExpandedWidth, screenWidth));
        return Rect.Union(targetScreen, TargetScreenBounds(expandedWidth, LargeHeight));
    }

    private Rect LocalBounds(Rect screen, Rect host)
    {
        var dpi = VisualTreeHelper.GetDpi(this);
        // Ukuran memakai nilai DIP asli agar tidak bergeser sepersekian DIP karena pembulatan piksel.
        return new Rect((screen.X - host.X) / dpi.DpiScaleX, (screen.Y - host.Y) / dpi.DpiScaleY,
            _sizeTargetWidth, _sizeTargetHeight);
    }

    private void SetHostBounds(Rect pixels)
    {
        var dpi = VisualTreeHelper.GetDpi(this);
        // HWND yang sudah tepat tidak disentuh: setiap SetWindowPos pada jendela transparan
        // bisa menampilkan satu frame lama sebelum WPF menggambar ulang (blinking).
        if (_windowHandle != IntPtr.Zero && DesktopNative.GetWindowRect(_windowHandle, out var current)
            && current.Left == (int)pixels.X && current.Top == (int)pixels.Y
            && current.Width == (int)pixels.Width && current.Height == (int)pixels.Height) return;
        _positioning = true;
        try
        {
            Width = pixels.Width / dpi.DpiScaleX; Height = pixels.Height / dpi.DpiScaleY;
            if (_windowHandle != IntPtr.Zero)
                DesktopNative.MoveWithoutActivating(_windowHandle, (int)pixels.X, (int)pixels.Y);
            else { Left = pixels.X / dpi.DpiScaleX; Top = pixels.Y / dpi.DpiScaleY; }
        }
        finally { _positioning = false; }
    }

    private void SnapNotchSize()
    {
        ++_sizeVersion;
        var targetScreen = TargetScreenBounds(_sizeTargetWidth, _sizeTargetHeight);
        var stage = StageScreenBounds(targetScreen);
        var local = LocalBounds(targetScreen, stage);
        NotchViewport.AnimateBounds(null);
        NotchViewport.VisibleBounds = local;
        ShellShift.BeginAnimation(TranslateTransform.XProperty, null);
        ShellShift.BeginAnimation(TranslateTransform.YProperty, null);
        ShellShift.X = ShellShift.Y = 0;
        ConfigurePanelLayout(local);
        _morphing = false;
        SetHostBounds(stage);
    }

    // Akhir morph: hanya lepaskan animasi ke nilai akhirnya. HWND sengaja tidak diubah,
    // karena di sinilah dulu jendela dikecilkan + digeser dan notch berkedip.
    private void SettleMorph(Rect target)
    {
        NotchViewport.VisibleBounds = target;
        NotchViewport.AnimateBounds(null);
        ShellShift.BeginAnimation(TranslateTransform.XProperty, null);
        ShellShift.BeginAnimation(TranslateTransform.YProperty, null);
        ShellShift.X = ShellShift.Y = 0;
        _morphing = false;
    }

    private void ConfigurePanelLayout(Rect target)
    {
        var screenWidth = ActiveScreenArea().Width / VisualTreeHelper.GetDpi(this).DpiScaleX - 20;
        var expandedWidth = Math.Max(1, Math.Min(ExpandedWidth, screenWidth));
        var layoutWidth = Math.Max(target.Width, expandedWidth);
        // Panel terbuka tetap punya layout utuh ketika menutup; clip yang mengecil, bukan tombolnya.
        Shell.Width = layoutWidth; Shell.Height = LargeHeight;
        Canvas.SetLeft(Shell, target.X + (target.Width - layoutWidth) / 2); Canvas.SetTop(Shell, target.Y);
        ExpandedHost.Width = Math.Max(1, expandedWidth - 26 - (_docked ? 2 * NotchSurface.MaxFlare : 0));
        if (!_expanded)
        {
            CompactPanel.Width = Math.Max(1, target.Width - CompactInset * 2);
            CompactPanel.Margin = new Thickness(0, (target.Height - CompactPanel.Height) / 2 - 9, 0, 0);
        }
    }

    private void MorphNotch(int milliseconds)
    {
        if (!SystemParameters.ClientAreaAnimation || _windowHandle == IntPtr.Zero
            || !DesktopNative.GetWindowRect(_windowHandle, out var window))
        { SnapNotchSize(); return; }
        var dpi = VisualTreeHelper.GetDpi(this);
        var oldContentCenter = window.Left + (Canvas.GetLeft(Shell) + Shell.Width / 2 + ShellShift.X) * dpi.DpiScaleX;
        var oldContentTop = window.Top + (Canvas.GetTop(Shell) + ShellShift.Y) * dpi.DpiScaleY;
        var visible = NotchViewport.VisibleBounds; // Nilai berjalan, aman saat hover berbalik arah.
        var oldScreen = new Rect(window.Left + visible.X * dpi.DpiScaleX,
            window.Top + visible.Y * dpi.DpiScaleY, visible.Width * dpi.DpiScaleX, visible.Height * dpi.DpiScaleY);
        var targetScreen = TargetScreenBounds(_sizeTargetWidth, _sizeTargetHeight);
        // Jendela hanya boleh membesar selama morph, tidak pernah mengecil:
        // gabungkan HWND sekarang, bentuk lama, dan panggung tujuan.
        var current = new Rect(window.Left, window.Top, window.Width, window.Height);
        var union = Rect.Union(Rect.Union(current, oldScreen), StageScreenBounds(targetScreen));
        var host = new Rect(Math.Floor(union.Left), Math.Floor(union.Top),
            Math.Ceiling(union.Right) - Math.Floor(union.Left), Math.Ceiling(union.Bottom) - Math.Floor(union.Top));
        Rect Local(Rect screen) => new((screen.X - host.X) / dpi.DpiScaleX, (screen.Y - host.Y) / dpi.DpiScaleY,
            screen.Width / dpi.DpiScaleX, screen.Height / dpi.DpiScaleY);
        var from = Local(oldScreen);
        var target = LocalBounds(targetScreen, host);
        var version = ++_sizeVersion;
        _morphing = true;
        NotchViewport.AnimateBounds(null);
        NotchViewport.VisibleBounds = from;
        SetHostBounds(host); // Ukuran buffer/jendela ditetapkan sekali sebelum transisi.
        ConfigurePanelLayout(target);
        var contentShift = (oldContentCenter - targetScreen.X - target.Width * dpi.DpiScaleX / 2) / dpi.DpiScaleX;
        ShellShift.BeginAnimation(TranslateTransform.XProperty, null); ShellShift.X = 0;
        ShellShift.BeginAnimation(TranslateTransform.YProperty, null); ShellShift.Y = 0;
        var slide = new DoubleAnimation(contentShift, 0, TimeSpan.FromMilliseconds(milliseconds))
        { EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }, FillBehavior = FillBehavior.HoldEnd };
        Timeline.SetDesiredFrameRate(slide, AnimationFrames);
        ShellShift.BeginAnimation(TranslateTransform.XProperty, slide, HandoffBehavior.SnapshotAndReplace);
        var slideY = new DoubleAnimation((oldContentTop - targetScreen.Y) / dpi.DpiScaleY, 0,
            TimeSpan.FromMilliseconds(milliseconds))
        { EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }, FillBehavior = FillBehavior.HoldEnd };
        Timeline.SetDesiredFrameRate(slideY, AnimationFrames);
        ShellShift.BeginAnimation(TranslateTransform.YProperty, slideY, HandoffBehavior.SnapshotAndReplace);
        var animation = new RectAnimation(from, target, TimeSpan.FromMilliseconds(milliseconds))
        {
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
            FillBehavior = FillBehavior.HoldEnd
        };
        Timeline.SetDesiredFrameRate(animation, AnimationFrames);
        animation.Completed += (_, _) =>
        {
            if (!_closed && !_minimizedToTray && version == _sizeVersion) SettleMorph(target);
        };
        NotchViewport.AnimateBounds(animation);
    }

    private void NotchMenu_Opened(object sender, RoutedEventArgs e)
    {
        _collapseTimer.Stop();
        NotchMenu.Items.Clear();

        NotchMenu.Items.Add(new MenuItem { Header = "NotchMusic v" + AppInfo.Version, IsEnabled = false });
        NotchMenu.Items.Add(new Separator());

        var automatic = new MenuItem
        {
            Header = "Otomatis: ikuti pemutar aktif Windows",
            IsCheckable = true,
            IsChecked = _manualSession is null
        };
        automatic.Click += (_, _) => { _manualSession = null; SelectSession(true); };
        NotchMenu.Items.Add(automatic);

        try
        {
            if (_manager is not null)
            {
                foreach (var session in _manager.GetSessions())
                {
                    var choice = new MenuItem
                    {
                        Header = FriendlySource(session.SourceAppUserModelId),
                        IsCheckable = true,
                        IsChecked = Equals(session, _manualSession)
                    };
                    choice.Click += (_, _) => { _manualSession = session; SelectSession(true); };
                    NotchMenu.Items.Add(choice);
                }
            }
        }
        catch (Exception ex) { Debug.WriteLine(ex); }

        NotchMenu.Items.Add(new Separator());
        var pin = new MenuItem { Header = "Selalu di atas", IsCheckable = true, IsChecked = Topmost };
        pin.Click += PinButton_Click;
        NotchMenu.Items.Add(pin);
        var reset = new MenuItem { Header = "Reset posisi ke atas tengah layar" };
        reset.Click += (_, _) => ResetPosition(); NotchMenu.Items.Add(reset);
        var minimize = new MenuItem { Header = "Minimalkan ke tray" };
        minimize.Click += (_, _) => MinimizeToTray(); NotchMenu.Items.Add(minimize);
        var light = new MenuItem { Header = "Mode hemat animasi (30 fps)", IsCheckable = true,
            IsChecked = _preferences.LightAnimations };
        light.Click += (_, _) =>
        {
            _preferences.LightAnimations = light.IsChecked;
            SnapNotchSize(); SetTransitionImmediately(); StopLyricScroll();
            SetEqualizerAnimation(false); UpdateCompactState(); SavePreferences();
        };
        NotchMenu.Items.Add(light);
        var editTheme = new MenuItem { Header = "Edit tema…" };
        editTheme.Click += (_, _) => OpenThemeEditor();
        NotchMenu.Items.Add(editTheme);
        var autostart = new MenuItem { Header = "Jalankan saat Windows mulai", IsCheckable = true, IsChecked = AutoStart.Enabled };
        autostart.Click += (_, _) =>
        {
            if (!AutoStart.Set(autostart.IsChecked)) StatusText.Text = "Autostart belum bisa diubah.";
        };
        NotchMenu.Items.Add(autostart);
        NotchMenu.Items.Add(new Separator());
        var refresh = new MenuItem { Header = "Baca ulang media" };
        refresh.Click += async (_, _) => await InitializeMediaAsync();
        NotchMenu.Items.Add(refresh);
        var memory = new MenuItem { Header = "Cek penggunaan RAM" };
        memory.Click += (_, _) => ShowMemoryUsage();
        NotchMenu.Items.Add(memory);
        var exit = new MenuItem { Header = "Keluar" };
        exit.Click += (_, _) => Close();
        NotchMenu.Items.Add(exit);
    }

    private void NotchMenu_Closed(object sender, RoutedEventArgs e)
    {
        NotchMenu.Items.Clear(); // Lepaskan referensi sesi yang ditangkap oleh menu.
        NotchMenu.PlacementTarget = Shell;
        NotchMenu.Placement = PlacementMode.MousePoint;
        if (!_closed && !_minimizedToTray && !_repositioning && !IsMouseOver && !_seekDragging) _collapseTimer.Start();
    }

    private void ShowMemoryUsage()
    {
        using var process = Process.GetCurrentProcess();
        process.Refresh();
        const double mb = 1024 * 1024;
        var graphics = RenderCapability.Tier >> 16 == 0 ? "CPU (tanpa GPU)" : "GPU";
        var animation = !SystemParameters.ClientAreaAnimation ? "animasi dimatikan Windows"
            : LightAnimations ? "mode hemat 30 fps" : $"animasi {AnimationFrames} fps";
        var report = $"NotchMusic v{AppInfo.Version}\n\n"
            + $"RAM (seperti Task Manager): {DesktopNative.PrivateWorkingSet(process) / mb:F1} MB\n"
            + $"Total termasuk file bersama: {process.WorkingSet64 / mb:F0} MB\n"
            + $"Puncak sejak dibuka: {process.PeakWorkingSet64 / mb:F0} MB\n\n"
            + $"Grafis: {graphics} · {animation}";
        MessageBox.Show(this, report, "Penggunaan RAM", MessageBoxButton.OK, MessageBoxImage.Information);
    }

    private void Window_PreviewKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape) { e.Handled = true; MinimizeToTray(); }
    }

    private void DetachSession()
    {
        if (_session is null) return;
        try
        {
            _session.MediaPropertiesChanged -= Session_MediaPropertiesChanged;
            _session.PlaybackInfoChanged -= Session_PlaybackInfoChanged;
            _session.TimelinePropertiesChanged -= Session_TimelinePropertiesChanged;
        }
        catch (Exception ex) { Debug.WriteLine(ex); }
    }

    private DesktopNative.WinEventProc? _foregroundChanged; // Disimpan agar delegate tidak dibuang GC.
    private IntPtr _foregroundHook;

    private void Window_Closed(object? sender, EventArgs e)
    {
        _closed = true;
        if (_foregroundHook != IntPtr.Zero) { DesktopNative.UnhookWinEvent(_foregroundHook); _foregroundHook = IntPtr.Zero; }
        StopClipboardFeature();
        FinishWindowDrag(resumeHover: false);
        try { _windowSource?.RemoveHook(Window_Hook); } catch (InvalidOperationException) { }
        _windowSource = null;
        _coverRefreshTimer.Stop();
        _metadataRetryTimer.Stop(); _skipFromTrackKey = null;
        _tray?.Dispose(); _tray = null;
        SetEqualizerAnimation(false);
        _metadataVersion++;
        _metadataPending = false;
        _collapseTimer.Stop();
        _collapseTimer.Tick -= CollapseTimer_Tick;
        _progressTimer.Stop();
        _progressTimer.Tick -= ProgressTimer_Tick;
        _clockTimer.Stop();
        _clockTimer.Tick -= ClockTimer_Tick;
        _lyricTimer.Stop(); _lyricTimer.Tick -= LyricTimer_Tick;
        _lyricsVersion++; _lyricsCts?.Cancel();
        _lyricDocument = null; _lyricsResult = null; _lyricChoice = null;
        _transitionVersion++; _sizeVersion++; _lyricAnimationVersion++;
        NotchViewport.AnimateBounds(null);
        SetTransitionImmediately();
        ShellShift.BeginAnimation(TranslateTransform.XProperty, null);
        ShellShift.BeginAnimation(TranslateTransform.YProperty, null);
        CompactPanel.BeginAnimation(OpacityProperty, null); ExpandedHost.BeginAnimation(OpacityProperty, null);
        StopLyricScroll();
        UpdateLyricTimer();
        _lyricsWindow?.Close();
        _dragSession = null;
        SystemParameters.StaticPropertyChanged -= SystemParameters_Changed;
        DetachSession();
        if (_manager is not null)
        {
            _manager.CurrentSessionChanged -= Manager_CurrentSessionChanged;
            _manager.SessionsChanged -= Manager_SessionsChanged;
        }
        _session = _manualSession = null;
        _manager = null;
        CoverImage.Source = null;
    }
}

// Geometry dipakai bersama oleh gambar dan visual clip, lalu dianimasikan langsung.
// Tidak ada Window.Width, Window.Height, SetWindowPos, atau UpdateLayout pada setiap frame.
public sealed class NotchSurface : Canvas
{
    private readonly RectangleGeometry _boundsClock = new(new Rect(0, 0, 128, 34));
    public const double MaxFlare = 14;
    private readonly TranslateTransform _shapeClock = new(1, 0);
    private readonly PathGeometry _visibleClip = new();
    private readonly PathFigure _figure = new() { IsClosed = true, IsFilled = true };
    private readonly LineSegment _top = new(), _right = new(), _bottom = new(), _left = new();
    private readonly BezierSegment _topRight = new(), _bottomRight = new(), _bottomLeft = new(), _topLeft = new();
    private int _shapeVersion;
    private double _dockTarget = 1;
    private Brush _fill = Brushes.Transparent;
    private Pen? _outline;
    // Garis progres: separuh kanan (tengah bawah -> tengah atas), separuh kiri = cerminannya.
    private readonly PathGeometry _progressHalf = new();
    private readonly PathFigure _progressFigure = new() { IsClosed = false, IsFilled = false };
    private readonly LineSegment _progressBottom = new(), _progressSide = new(), _progressTop = new();
    private readonly BezierSegment _progressLower = new(), _progressUpper = new();
    private readonly ScaleTransform _mirror = new(-1, 1);
    private Pen? _progressPen;
    private double _progress = -1; // Negatif = tersembunyi.
    private double _halfLength;

    public Rect VisibleBounds
    {
        get => _boundsClock.Rect;
        set => _boundsClock.Rect = value;
    }

    public bool ContainsPoint(Point point) => _visibleClip.FillContains(point);

    public NotchSurface()
    {
        _figure.Segments = new PathSegmentCollection
            { _top, _topRight, _right, _bottomRight, _bottom, _bottomLeft, _left, _topLeft };
        _visibleClip.Figures.Add(_figure);
        _progressFigure.Segments = new PathSegmentCollection
            { _progressBottom, _progressLower, _progressSide, _progressUpper, _progressTop };
        _progressHalf.Figures.Add(_progressFigure);
        _boundsClock.Changed += (_, _) => UpdateOutline();
        _shapeClock.Changed += (_, _) => UpdateOutline();
        UpdateOutline();
    }

    public void SetDockCut(bool halfOval, bool animate, int milliseconds, int frames)
    {
        var target = halfOval ? 1d : 0d;
        if (animate && target == _dockTarget) return;
        _dockTarget = target;
        var from = _shapeClock.X;
        var version = ++_shapeVersion;
        _shapeClock.BeginAnimation(TranslateTransform.XProperty, null);
        _shapeClock.X = target;
        if (!animate || Math.Abs(from - target) < 0.001) return;
        var animation = new DoubleAnimation(from, target, TimeSpan.FromMilliseconds(milliseconds))
        { EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }, FillBehavior = FillBehavior.Stop };
        Timeline.SetDesiredFrameRate(animation, frames);
        animation.Completed += (_, _) =>
        { if (version == _shapeVersion) _shapeClock.BeginAnimation(TranslateTransform.XProperty, null); };
        _shapeClock.BeginAnimation(TranslateTransform.XProperty, animation, HandoffBehavior.SnapshotAndReplace);
    }

    private void UpdateOutline()
    {
        var b = _boundsClock.Rect;
        var cut = Math.Clamp(_shapeClock.X, 0, 1);
        // Menempel: sudut atas melengkung ke luar (seperti notch MacBook) agar menyatu dengan tepi layar.
        // Badan notch menyempit sebesar flare di kiri-kanan; cut 0..1 memorf antara oval dan bentuk ini.
        var flare = Math.Min(MaxFlare, b.Width / 6) * cut;
        var x = b.Left; var y = b.Top; var r = b.Right; var d = b.Bottom;
        var left = x + flare; var right = r - flare;
        var radius = Math.Min(24, Math.Min((right - left) / 2, b.Height / 2));
        var upper = radius * (1 - cut);
        var lower = Math.Min(b.Height - upper - flare, Math.Min((right - left) / 2, radius * (1 + cut)));
        const double k = 0.5522847498307936; // Pendekatan seperempat lingkaran dengan Bezier kubik.
        // Satu Bezier per sudut atas: cembung (upper) saat mengambang, cekung (flare) saat menempel.
        _figure.StartPoint = new Point(x + upper, y);
        _top.Point = new Point(r - upper, y);
        _topRight.Point1 = new Point(r - upper + k * upper - k * flare, y);
        _topRight.Point2 = new Point(right, y + upper - k * upper + flare - k * flare);
        _topRight.Point3 = new Point(right, y + upper + flare);
        _right.Point = new Point(right, d - lower);
        _bottomRight.Point1 = new Point(right, d - lower + k * lower);
        _bottomRight.Point2 = new Point(right - lower + k * lower, d);
        _bottomRight.Point3 = new Point(right - lower, d);
        _bottom.Point = new Point(left + lower, d);
        _bottomLeft.Point1 = new Point(left + lower - k * lower, d);
        _bottomLeft.Point2 = new Point(left, d - lower + k * lower);
        _bottomLeft.Point3 = new Point(left, d - lower);
        _left.Point = new Point(left, y + upper + flare);
        _topLeft.Point1 = new Point(left, y + upper - k * upper + flare - k * flare);
        _topLeft.Point2 = new Point(x + upper - k * upper + k * flare, y);
        _topLeft.Point3 = _figure.StartPoint;

        var center = (x + r) / 2;
        _progressFigure.StartPoint = new Point(center, d);
        _progressBottom.Point = new Point(right - lower, d);
        _progressLower.Point1 = new Point(right - lower + k * lower, d);
        _progressLower.Point2 = new Point(right, d - lower + k * lower);
        _progressLower.Point3 = new Point(right, d - lower);
        _progressSide.Point = new Point(right, y + upper + flare);
        _progressUpper.Point1 = _topRight.Point2;
        _progressUpper.Point2 = _topRight.Point1;
        _progressUpper.Point3 = new Point(r - upper, y);
        _progressTop.Point = new Point(center, y);
        _mirror.CenterX = center;
        // Saat menempel (cut=1) sisi atas berada di tepi layar dan tak terlihat: jangan dihitung,
        // agar garis baru penuh di ujung lengkung atas tepat ketika lagu selesai.
        _halfLength = Math.Max(0, right - lower - center) + Math.PI / 2 * lower
            + Math.Max(0, d - lower - y - upper - flare) + Math.PI / 2 * (upper + flare)
            + Math.Max(0, r - upper - center) * (1 - cut);
        UpdateProgressDash();
    }

    public void SetProgressBrush(Brush brush)
    {
        // Setengah tebal garis terpotong clip, jadi yang tampak sekitar 1,5 DIP.
        _progressPen = new Pen(brush, 3)
        {
            StartLineCap = PenLineCap.Flat, EndLineCap = PenLineCap.Flat, DashCap = PenLineCap.Flat,
            DashStyle = new DashStyle(new[] { 0d, 100000d }, 0)
        };
        UpdateProgressDash();
        InvalidateVisual();
    }

    // fraction 0..1; visible=false menyembunyikan garis (mis. saat notch terbuka).
    public void SetProgress(double fraction, bool visible)
    {
        var value = visible && double.IsFinite(fraction) ? Math.Clamp(fraction, 0, 1) : -1;
        var toggled = value < 0 != _progress < 0;
        _progress = value;
        UpdateProgressDash();
        if (toggled) InvalidateVisual();
    }

    private void UpdateProgressDash()
    {
        if (_progressPen is null || _progress < 0) return;
        var dash = _progress * _halfLength / _progressPen.Thickness; // Satuan dash = kelipatan tebal pen.
        if (Math.Abs(_progressPen.DashStyle.Dashes[0] - dash) <= 0.01) return;
        _progressPen.DashStyle.Dashes[0] = dash;
        // Perubahan dash pada pen yang sudah terekam tidak sampai ke layar; rekam ulang gambarnya.
        InvalidateVisual();
    }

    // Kembalikan geometry yang sama; jangan mengganti UIElement.Clip pada setiap frame.
    protected override Geometry GetLayoutClip(System.Windows.Size layoutSlotSize) => _visibleClip;

    public void AnimateBounds(RectAnimation? animation)
        => _boundsClock.BeginAnimation(RectangleGeometry.RectProperty, animation, HandoffBehavior.SnapshotAndReplace);

    public void SetPalette(Brush fill, Brush outline)
    {
        _fill = fill;
        _outline = new Pen(outline, 1.2); // Clip menyisakan garis tipis sekitar 0,6 DIP.
        if (_outline.CanFreeze) _outline.Freeze();
        InvalidateVisual();
    }

    protected override void OnRender(DrawingContext dc)
    {
        base.OnRender(dc);
        // Drawing mempertahankan referensi geometry; perubahan Rect tidak membangun ulang isi UI.
        dc.DrawGeometry(_fill, _outline, _visibleClip);
        if (_progressPen is null || _progress < 0) return;
        dc.DrawGeometry(null, _progressPen, _progressHalf);
        dc.PushTransform(_mirror);
        dc.DrawGeometry(null, _progressPen, _progressHalf);
        dc.Pop();
    }
}

internal sealed class NotchPreferences
{
    public NotchPreferences() { }
    public bool PinOnTop { get; set; } = true;
    public int Theme { get; set; }
    public string[]? CustomColors { get; set; }
    public List<string>? CustomGames { get; set; } // Versi lama; digabung ke GameOrder saat dibaca.
    public List<string> GameOrder { get; set; } = []; // Daftar game di notch (appid Steam atau path shortcut), sesuai urutan.
    public bool LightAnimations { get; set; }
    public int ExpandedPage { get; set; }
    public bool ClipboardRecordingEnabled { get; set; } = true;
    public bool DockedToTop { get; set; } = true;
    public bool HasPosition { get; set; }
    public double CenterX { get; set; }
    public double TopY { get; set; }
    private static string FilePath => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "NotchMusic", "ui-settings.json");

    public static NotchPreferences Load()
    {
        try
        {
            if (!File.Exists(FilePath) || new FileInfo(FilePath).Length > 65536) return new();
            var value = JsonSerializer.Deserialize<NotchPreferences>(File.ReadAllText(FilePath)) ?? new();
            if (!double.IsFinite(value.CenterX) || !double.IsFinite(value.TopY)
                || Math.Abs(value.CenterX) > 1000000 || Math.Abs(value.TopY) > 1000000)
                value.HasPosition = false;
            // File bisa disunting manual: tema kustom hanya dipakai bila berisi 5-6 kode #RRGGBB.
            if (value.CustomColors is { } colors && (colors.Length is not (5 or 6) || !colors.All(ThemeEditor.IsHex)))
                value.CustomColors = null;
            // Daftar game tambahan berasal dari file yang bisa disunting: batasi jumlah dan panjang path.
            value.GameOrder = (value.GameOrder ?? []).Concat(value.CustomGames ?? [])
                .Where(x => x is not null && (SteamLibrary.IsAppId(x) || x.Length <= 1024 && Path.IsPathFullyQualified(x)))
                .Distinct(StringComparer.OrdinalIgnoreCase).Take(MainWindow.GameSlots - 1).ToList();
            value.CustomGames = null;
            return value;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        { Debug.WriteLine(ex); return new(); }
    }

    public bool Save()
    {
        try
        {
            var path = FilePath;
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(this));
            File.Move(path + ".tmp", path, true);
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        { Debug.WriteLine(ex); return false; }
    }
}

// Tray native: tidak memerlukan Windows Forms, paket NuGet, atau polling permanen.
internal sealed class NativeTrayIcon : IDisposable
{
    private const int CallbackMessage = 0x805B;
    private readonly IntPtr _handle;
    private readonly HwndSource? _source;
    private readonly Action _restore;
    private readonly Action _menu;
    private readonly Action _recover;
    private readonly uint _taskbarCreated;
    private bool _added;
    private bool _version4;
    private bool _disposed;

    // Ikon mint dari file; bila hilang pakai ikon aplikasi bersama milik Windows (tidak boleh di-DestroyIcon).
    private readonly IntPtr _icon;
    private readonly bool _ownsIcon;

    public NativeTrayIcon(IntPtr handle, Action restore, Action menu, Action recover)
    {
        _handle = handle; _restore = restore; _menu = menu; _recover = recover;
        var size = GetSystemMetrics(49); // SM_CXSMICON, sudah mengikuti DPI proses.
        _icon = LoadImage(IntPtr.Zero, Path.Combine(AppContext.BaseDirectory, "Assets", "NotchMusic_Tray_Mint.ico"),
            1, size, size, 0x10); // IMAGE_ICON, LR_LOADFROMFILE
        _ownsIcon = _icon != IntPtr.Zero;
        if (!_ownsIcon) _icon = LoadIcon(IntPtr.Zero, new IntPtr(32512));
        _source = HwndSource.FromHwnd(handle);
        _taskbarCreated = RegisterWindowMessage("TaskbarCreated");
        _source?.AddHook(Hook);
        EnsureVisible();
    }

    private NotifyIconData Data() => new()
    {
        Size = (uint)Marshal.SizeOf<NotifyIconData>(), Window = _handle, Id = 1,
        Flags = 0x01 | 0x02 | 0x04 | 0x80, Callback = CallbackMessage,
        Icon = _icon,
        Tip = "NotchMusic — klik untuk tampilkan, klik kanan untuk menu",
        Info = "", InfoTitle = "", Version = 4
    };

    public bool EnsureVisible()
    {
        if (_disposed || _source is null) return false;
        if (_added) return true;
        var data = Data();
        _added = ShellNotifyIcon(0, ref data); // NIM_ADD
        if (_added) _version4 = ShellNotifyIcon(4, ref data); // NIM_SETVERSION
        return _added;
    }

    private IntPtr Hook(IntPtr hwnd, int message, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (_disposed) return IntPtr.Zero;
        if (_taskbarCreated != 0 && (uint)message == _taskbarCreated)
        {
            _added = false;
            if (!EnsureVisible()) _recover();
        }
        if (message != CallbackMessage) return IntPtr.Zero;
        var code = (int)(lParam.ToInt64() & 0xffff);
        if (code is 0x0400 or 0x0401 || (!_version4 && (code is 0x0202 or 0x0203)))
        { handled = true; _restore(); }
        else if (code == 0x007B || (!_version4 && code == 0x0205))
        { handled = true; _menu(); }
        return IntPtr.Zero;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (_added) { var data = Data(); ShellNotifyIcon(2, ref data); _added = false; }
        try { _source?.RemoveHook(Hook); } catch (InvalidOperationException) { }
        if (_ownsIcon) DestroyIcon(_icon);
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData
    {
        public uint Size;
        public IntPtr Window;
        public uint Id, Flags, Callback;
        public IntPtr Icon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string Tip;
        public uint State, StateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string Info;
        public uint Version;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string InfoTitle;
        public uint InfoFlags;
        public Guid Guid;
        public IntPtr BalloonIcon;
    }

    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconW", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData data);
    [DllImport("user32.dll", EntryPoint = "RegisterWindowMessageW", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessage(string message);
    [DllImport("user32.dll", EntryPoint = "LoadIconW")]
    private static extern IntPtr LoadIcon(IntPtr instance, IntPtr name);
    [DllImport("user32.dll", EntryPoint = "LoadImageW", CharSet = CharSet.Unicode)]
    private static extern IntPtr LoadImage(IntPtr instance, string name, uint type, int width, int height, uint flags);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyIcon(IntPtr icon);
    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int index);
}

// Editor tema kustom. Kotak warna membuka dialog warna Windows (sama dengan "Edit colors" di Paint).
internal sealed class ThemeEditor : Window
{
    private static readonly string[] Labels = ["Latar notch", "Cover dan garis tepi", "Tombol", "Aksen (hover, progres, lirik)", "Ikon di tombol",
        "Garis progres notch"];
    private readonly MainWindow _main;
    private readonly string[] _colors;
    private readonly TextBox[] _hex = new TextBox[6];
    private readonly Button[] _swatch = new Button[6];
    private bool _saved;

    public static bool IsHex(string? text) => text is not null && Regex.IsMatch(text, "^#[0-9A-Fa-f]{6}$");

    public ThemeEditor(MainWindow main, string[] colors)
    {
        _main = main; _colors = colors.Length > 5 ? (string[])colors.Clone() : [.. colors, colors[3]];
        Title = "Edit tema"; Owner = main; Topmost = main.Topmost; ShowInTaskbar = false;
        ResizeMode = ResizeMode.NoResize; SizeToContent = SizeToContent.WidthAndHeight;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        var root = new StackPanel { Margin = new Thickness(16) };
        root.Children.Add(new TextBlock { Text = "Klik kotak warna untuk memilih, atau ketik kode hex.", Margin = new Thickness(0, 0, 0, 12) });
        for (var i = 0; i < Labels.Length; i++)
        {
            var index = i;
            _swatch[i] = new Button { Width = 36, Height = 24, Margin = new Thickness(0, 0, 10, 0), ToolTip = "Pilih warna" };
            _hex[i] = new TextBox { Width = 84, MaxLength = 7, Text = _colors[i], VerticalContentAlignment = VerticalAlignment.Center };
            AutomationProperties.SetName(_swatch[i], "Pilih warna " + Labels[i]);
            AutomationProperties.SetName(_hex[i], "Kode hex " + Labels[i]);
            _swatch[i].Click += (_, _) => Pick(index);
            _hex[i].TextChanged += (_, _) => Typed(index);
            Paint(index);
            var row = new DockPanel { Margin = new Thickness(0, 0, 0, 8) };
            DockPanel.SetDock(_swatch[i], Dock.Left); DockPanel.SetDock(_hex[i], Dock.Right);
            row.Children.Add(_swatch[i]); row.Children.Add(_hex[i]);
            row.Children.Add(new TextBlock { Text = Labels[i], MinWidth = 190, VerticalAlignment = VerticalAlignment.Center });
            root.Children.Add(row);
        }
        var save = new Button { Content = "Simpan", Padding = new Thickness(14, 4, 14, 4), IsDefault = true };
        var cancel = new Button { Content = "Batal", Padding = new Thickness(14, 4, 14, 4), Margin = new Thickness(8, 0, 0, 0), IsCancel = true };
        save.Click += (_, _) => { _saved = true; _main.SaveCustomTheme((string[])_colors.Clone()); Close(); };
        cancel.Click += (_, _) => Close();
        root.Children.Add(new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right,
            Margin = new Thickness(0, 8, 0, 0), Children = { save, cancel } });
        Content = root;
        Closed += (_, _) => { if (!_saved) _main.CancelThemeEdit(); }; // Pratinjau dibatalkan.
    }

    private void Paint(int i)
    {
        var brush = new SolidColorBrush((Color)ColorConverter.ConvertFromString(_colors[i]));
        brush.Freeze();
        _swatch[i].Background = brush;
    }

    private void Typed(int i)
    {
        var text = _hex[i].Text.Trim();
        if (!text.StartsWith('#')) text = "#" + text;
        if (!IsHex(text)) return; // Tunggu sampai kode lengkap.
        _colors[i] = text.ToUpperInvariant();
        Paint(i); _main.ApplyColors(_colors); // Pratinjau langsung di notch.
    }

    private void Pick(int i)
    {
        var start = (Color)ColorConverter.ConvertFromString(_colors[i]);
        if (ColorDialog.Pick(new WindowInteropHelper(this).Handle, start) is { } color)
            _hex[i].Text = $"#{color.R:X2}{color.G:X2}{color.B:X2}";
    }
}

internal static class ColorDialog
{
    private static readonly int[] Custom = new int[16]; // Warna kustom dialog diingat selama aplikasi berjalan.

    [StructLayout(LayoutKind.Sequential)]
    private struct ChooseColorData
    {
        public int Size;
        public IntPtr Owner, Instance;
        public int Rgb;
        public IntPtr CustomColors;
        public int Flags;
        public IntPtr CustomData, Hook, Template;
    }

    public static Color? Pick(IntPtr owner, Color start)
    {
        var pinned = GCHandle.Alloc(Custom, GCHandleType.Pinned);
        try
        {
            var data = new ChooseColorData
            {
                Size = Marshal.SizeOf<ChooseColorData>(), Owner = owner, Rgb = start.R | start.G << 8 | start.B << 16,
                CustomColors = pinned.AddrOfPinnedObject(), Flags = 0x1 | 0x2 | 0x100 // CC_RGBINIT, CC_FULLOPEN, CC_ANYCOLOR
            };
            if (!ChooseColor(ref data)) return null;
            return Color.FromRgb((byte)data.Rgb, (byte)(data.Rgb >> 8), (byte)(data.Rgb >> 16));
        }
        finally { pinned.Free(); }
    }

    [DllImport("comdlg32.dll", EntryPoint = "ChooseColorW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ChooseColor(ref ChooseColorData data);
}

// Satu sumber versi: <Version> di NotchMusic.csproj.
internal static class AppInfo
{
    public static readonly string Version = typeof(AppInfo).Assembly.GetName().Version?.ToString(3) ?? "?";
}

// Autostart per pengguna lewat HKCU\...\Run: tanpa admin, registry adalah satu-satunya sumber status.
internal static class AutoStart
{
    private const string Key = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string Name = "NotchMusic";
    private static string Command => $"\"{Environment.ProcessPath}\"";

    // Dianggap aktif hanya bila menunjuk exe ini; folder dipindah = mati, centang ulang memperbaikinya.
    public static bool Enabled
    {
        get
        {
            try
            {
                using var run = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(Key);
                return string.Equals(run?.GetValue(Name) as string, Command, StringComparison.OrdinalIgnoreCase);
            }
            catch (Exception ex) when (ex is System.Security.SecurityException or UnauthorizedAccessException or IOException)
            { Debug.WriteLine(ex); return false; }
        }
    }

    public static bool Set(bool enabled)
    {
        try
        {
            using var run = Microsoft.Win32.Registry.CurrentUser.CreateSubKey(Key);
            if (enabled) run.SetValue(Name, Command); else run.DeleteValue(Name, false);
            return true;
        }
        catch (Exception ex) when (ex is System.Security.SecurityException or UnauthorizedAccessException or IOException)
        { Debug.WriteLine(ex); return false; }
    }
}

// Perhitungan murni: memakai posisi mouse absolut, bukan hasil snap sebelumnya.
// Ini mencegah snap di atas terus mengunci gerakan ke bawah.
internal static class NotchPlacement
{
    internal static (int Left, int Top, bool Docked) Resolve(double cursorX, double cursorY,
        double grabX, double grabY, int width, int height, DesktopNative.NativeRect screen, double dpiY, bool wasDocked)
    {
        var rawLeft = cursorX - grabX * width;
        var rawTop = cursorY - grabY * height;
        var docked = rawTop <= screen.Top + (wasDocked ? 12 : 6) * dpiY;
        var left = Math.Clamp(rawLeft, screen.Left, Math.Max(screen.Left, screen.Right - width));
        var top = docked ? screen.Top
            : Math.Clamp(rawTop, screen.Top, Math.Max(screen.Top, screen.Bottom - height));
        return ((int)Math.Round(left), (int)Math.Round(top), docked);
    }
}

internal static class DesktopNative
{
    internal static readonly uint RestoreMessage = RegisterWindowMessage("NotchMusic.Restore.v1");
    [StructLayout(LayoutKind.Sequential)]
    internal struct NativeRect
    {
        public int Left, Top, Right, Bottom;
        public int Width => Right - Left;
        public int Height => Bottom - Top;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativePoint { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo { public uint Size; public NativeRect Monitor, Work; public uint Flags; }

    internal static NativeRect ScreenArea(IntPtr window, bool custom, double centerX, double top)
    {
        var monitor = custom ? MonitorFromPoint(new NativePoint { X = (int)Math.Round(centerX), Y = (int)Math.Round(top) }, 2)
            : MonitorFromWindow(window, 2); // Pilih monitor terdekat jika layar lama dilepas.
        var info = new MonitorInfo { Size = (uint)Marshal.SizeOf<MonitorInfo>() };
        if (GetMonitorInfo(monitor, ref info)) return info.Monitor;
        return new NativeRect { Left = 0, Top = 0, Right = (int)SystemParameters.PrimaryScreenWidth,
            Bottom = (int)SystemParameters.PrimaryScreenHeight };
    }

    internal static bool TryGetCursor(out Point cursor)
    {
        var ok = GetCursorPos(out var point);
        cursor = new Point(point.X, point.Y);
        return ok;
    }

    internal static void RestoreExistingNotch()
    {
        var window = FindWindow(null, "NotchMusic");
        if (window != IntPtr.Zero && RestoreMessage != 0) PostMessage(window, RestoreMessage, IntPtr.Zero, IntPtr.Zero);
    }

    // Jendela depan milik aplikasi lain menutupi seluruh monitor notch: game atau aplikasi layar penuh.
    // Jendela maximize ber-title bar tidak dihitung (dengan taskbar auto-hide ia juga menutupi monitor);
    // game borderless sering juga ber-status maximize tetapi tanpa title bar.
    internal static bool FullscreenAppOnMonitor(IntPtr self)
    {
        var foreground = GetForegroundWindow();
        if (self == IntPtr.Zero || foreground == IntPtr.Zero || foreground == GetShellWindow()) return false;
        const long caption = 0x00C00000; // WS_CAPTION
        if (IsZoomed(foreground) && (GetWindowLongPtr(foreground, -16).ToInt64() & caption) == caption) return false; // GWL_STYLE
        GetWindowThreadProcessId(foreground, out var process);
        if (process == (uint)Environment.ProcessId) return false;
        var name = new StringBuilder(16);
        GetClassName(foreground, name, name.Capacity);
        if (name.ToString() is "WorkerW" or "Progman") return false; // Desktop.
        var monitor = MonitorFromWindow(self, 2);
        var info = new MonitorInfo { Size = (uint)Marshal.SizeOf<MonitorInfo>() };
        if (MonitorFromWindow(foreground, 2) != monitor || !GetMonitorInfo(monitor, ref info)
            || !GetWindowRect(foreground, out var window)) return false;
        return window.Left <= info.Monitor.Left && window.Top <= info.Monitor.Top
            && window.Right >= info.Monitor.Right && window.Bottom >= info.Monitor.Bottom;
    }

    // shortcut: membaca refresh layar utama saja, cukup selama notch ada di layar utama; pindah ke
    // EnumDisplaySettings per-monitor bila notch sering di monitor lain dengan refresh berbeda.
    internal static int PrimaryRefreshRate()
    {
        var mode = new DevMode { Size = (ushort)Marshal.SizeOf<DevMode>() };
        return EnumDisplaySettings(null, -1, ref mode) && mode.DisplayFrequency > 1 ? (int)mode.DisplayFrequency : 60; // ENUM_CURRENT_SETTINGS
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct DevMode
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
        public ushort SpecVersion, DriverVersion, Size, DriverExtra;
        public uint Fields;
        public int PositionX, PositionY;
        public uint DisplayOrientation, DisplayFixedOutput;
        public short Color, Duplex, YResolution, TTOption, Collate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string FormName;
        public ushort LogPixels;
        public uint BitsPerPel, PelsWidth, PelsHeight, DisplayFlags, DisplayFrequency;
        public uint IcmMethod, IcmIntent, MediaType, DitherType, Reserved1, Reserved2, PanningWidth, PanningHeight;
    }

    [DllImport("user32.dll", EntryPoint = "EnumDisplaySettingsW", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumDisplaySettings(string? device, int mode, ref DevMode devMode);

    // WS_EX_TOOLWINDOW: tidak muncul di Alt+Tab (ShowInTaskbar=false saja tidak cukup).
    internal static void HideFromAltTab(IntPtr window)
        => SetWindowLongPtr(window, -20, new IntPtr(GetWindowLongPtr(window, -20).ToInt64() | 0x80)); // GWL_EXSTYLE

    internal delegate void WinEventProc(IntPtr hook, uint eventType, IntPtr window, int objectId, int childId, uint thread, uint time);

    // EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT: dipanggil di thread UI lewat antrean pesan, tanpa polling.
    internal static IntPtr WatchForeground(WinEventProc callback) => SetWinEventHook(3, 3, IntPtr.Zero, callback, 0, 0, 0);

    // HWND_TOPMOST tanpa memindah, mengubah ukuran, atau mengaktifkan.
    internal static void KeepOnTop(IntPtr window) => SetWindowPos(window, new IntPtr(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);

    [DllImport("user32.dll")]
    private static extern IntPtr SetWinEventHook(uint eventMin, uint eventMax, IntPtr module, WinEventProc callback, uint process, uint thread, uint flags);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool UnhookWinEvent(IntPtr hook);

    internal static void TrimWorkingSet() => EmptyWorkingSet(Process.GetCurrentProcess().Handle);

    [DllImport("psapi.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EmptyWorkingSet(IntPtr process);

    // Angka kolom "Memory" di Task Manager (private working set). Gagal: pakai working set biasa.
    internal static long PrivateWorkingSet(Process process)
    {
        var counters = new MemoryCounters { Size = (uint)Marshal.SizeOf<MemoryCounters>() };
        return GetProcessMemoryInfo(process.Handle, ref counters, counters.Size)
            ? (long)counters.PrivateWorkingSetSize : process.WorkingSet64;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MemoryCounters // PROCESS_MEMORY_COUNTERS_EX2 (Windows 10 1803+)
    {
        public uint Size, PageFaultCount;
        public nuint PeakWorkingSetSize, WorkingSetSize, QuotaPeakPagedPoolUsage, QuotaPagedPoolUsage,
            QuotaPeakNonPagedPoolUsage, QuotaNonPagedPoolUsage, PagefileUsage, PeakPagefileUsage, PrivateUsage,
            PrivateWorkingSetSize;
        public ulong SharedCommitUsage;
    }

    [DllImport("psapi.dll", EntryPoint = "GetProcessMemoryInfo")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetProcessMemoryInfo(IntPtr process, ref MemoryCounters counters, uint size);

    internal static void MoveWithoutActivating(IntPtr window, int x, int y)
        => SetWindowPos(window, IntPtr.Zero, x, y, 0, 0, 0x0001 | 0x0004 | 0x0010 | 0x0200);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool GetWindowRect(IntPtr window, out NativeRect rectangle);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out NativePoint point);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindWindow(string? className, string windowName);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessage(string message);
    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);
    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromPoint(NativePoint point, uint flags);
    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    private static extern IntPtr GetShellWindow();
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsZoomed(IntPtr window);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static extern IntPtr GetWindowLongPtr(IntPtr window, int index);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static extern IntPtr SetWindowLongPtr(IntPtr window, int index, IntPtr value);
    [DllImport("user32.dll", EntryPoint = "GetClassNameW", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr window, StringBuilder name, int capacity);
    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int cx, int cy, uint flags);
}

// Jendela ini memilih hasil/pengaturan. Baris lirik sehari-hari tampil langsung di notch.
internal sealed class LyricsWindow : Window
{
    private readonly MainWindow _main;
    private readonly TextBox _title = new();
    private readonly TextBox _artist = new();
    private readonly ComboBox _mode = new();
    private readonly ComboBox _results = new();
    private readonly TextBlock _heading = new();
    private readonly TextBlock _status = new();
    private readonly TextBlock _body = new();
    private readonly TextBlock _offsetText = new();
    private readonly Button _save;
    private readonly Button _load;
    private readonly Button _saved;
    private string? _key;
    private bool _setting;
    private LyricsSearchResult? _shownResult;
    private LyricDocument? _shownDocument;
    private bool _closed;

    public LyricsWindow(MainWindow main)
    {
        _main = main;
        Title = "Lirik · NotchMusic";
        Width = 480; Height = 620; MinWidth = 380; MinHeight = 420;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        Topmost = main.Topmost;
        FontFamily = new FontFamily("Segoe UI"); FontSize = 12;
        // Warna dan gaya tombol/scrollbar diambil dari notch, jadi ikut tema yang aktif.
        _buttonStyle = (Style)main.FindResource("MusicButton");
        Resources[typeof(ScrollBar)] = new Style(typeof(ScrollBar), (Style)main.FindResource("ClipboardScrollBar"));
        Resources[typeof(ComboBox)] = main.FindResource("ThemedComboBox");
        UseTheme(main.Resources);
        SetResourceReference(BackgroundProperty, "NotchBackground");
        Foreground = Brushes.White;
        var root = new StackPanel { Margin = new Thickness(18) };
        Content = new ScrollViewer { Content = root, VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };

        _heading.FontSize = 16; _heading.FontWeight = FontWeights.SemiBold; _heading.TextWrapping = TextWrapping.Wrap;
        root.Children.Add(_heading);
        _status.FontSize = 11; _status.Opacity = 0.8; _status.TextWrapping = TextWrapping.Wrap;
        _status.Margin = new Thickness(0, 2, 0, 14); root.Children.Add(_status);

        // Jenis lirik + cari ulang.
        var top = new DockPanel { Margin = new Thickness(0, 0, 0, 10) };
        var search = Button("Cari ulang");
        search.ToolTip = "Cari lirik lagu ini lagi di internet";
        search.Margin = new Thickness(8, 0, 0, 0);
        search.Click += (_, _) => Search();
        DockPanel.SetDock(search, Dock.Right); top.Children.Add(search);
        foreach (var text in new[] { "Otomatis", "Lirik asli", "Romaji (Jepang)" }) _mode.Items.Add(text);
        _mode.SelectedIndex = 0;
        _mode.ToolTip = "Otomatis: romaji untuk lagu Jepang, bahasa asli untuk lagu lain";
        AutomationProperties.SetName(_mode, "Jenis lirik");
        Field(_mode); top.Children.Add(_mode);
        root.Children.Add(top);

        // Hasil yang tampil di notch.
        root.Children.Add(Label("Lirik yang dipakai"));
        _results.DisplayMemberPath = nameof(LyricCandidate.DisplayName); _results.MaxDropDownHeight = 220;
        _results.ToolTip = "Persentase adalah kemiripan judul, bukan kepastian";
        AutomationProperties.SetName(_results, "Lirik yang dipakai");
        _results.SelectionChanged += (_, _) => { if (!_setting && _results.SelectedItem is LyricCandidate choice) _main.ChooseLyrics(choice); };
        Field(_results); _results.Margin = new Thickness(0, 0, 0, 10); root.Children.Add(_results);

        // Koreksi waktu.
        var timing = new WrapPanel { Margin = new Thickness(0, 0, 0, 12) };
        _offsetText.VerticalAlignment = VerticalAlignment.Center; _offsetText.MinWidth = 120;
        timing.Children.Add(_offsetText);
        var earlier = Button("Lebih awal");
        earlier.ToolTip = "Lirik muncul terlambat? Majukan 0,5 detik";
        earlier.Click += (_, _) => _main.SetLyricsOffset(_main.LyricsOffsetSeconds - 0.5); timing.Children.Add(earlier);
        var later = Button("Lebih lambat");
        later.ToolTip = "Lirik muncul terlalu cepat? Tunda 0,5 detik";
        later.Click += (_, _) => _main.SetLyricsOffset(_main.LyricsOffsetSeconds + 0.5); timing.Children.Add(later);
        var resetTiming = Button("Reset");
        resetTiming.Click += (_, _) => _main.SetLyricsOffset(0); timing.Children.Add(resetTiming);
        root.Children.Add(timing);

        // Jarang dipakai: judul/artis dan file.
        var more = new StackPanel { Margin = new Thickness(0, 8, 0, 4) };
        more.Children.Add(Label("Judul dan artis untuk pencarian"));
        foreach (var (name, input) in new[] { ("Judul", _title), ("Artis", _artist) })
        {
            input.MaxLength = 512; input.Margin = new Thickness(0, 0, 0, 6);
            AutomationProperties.SetName(input, name); Field(input);
            more.Children.Add(input);
        }
        var files = new WrapPanel { Margin = new Thickness(0, 4, 0, 12) };
        _save = Button("Simpan .lrc");
        _save.ToolTip = "Simpan lirik dan waktunya ke file";
        _save.Click += async (_, _) => await _main.ExportLyricsAsync(); files.Children.Add(_save);
        _load = Button("Muat file…");
        _load.ToolTip = "Pakai file .lrc atau .txt untuk lagu ini";
        _load.Click += async (_, _) => await _main.ImportLyricsAsync((LyricsMode)_mode.SelectedIndex); files.Children.Add(_load);
        _saved = Button("Lirik tersimpan…");
        _saved.ToolTip = "Pakai lirik yang pernah tersimpan untuk lagu ini";
        _saved.Click += async (_, _) => await _main.ShowSavedLyricsAsync(_title.Text.Trim(), _artist.Text.Trim(), (LyricsMode)_mode.SelectedIndex);
        files.Children.Add(_saved);
        more.Children.Add(files);

        root.Children.Add(new Expander { Header = "Lainnya: judul/artis dan file", Content = more,
            Foreground = Brushes.White, Margin = new Thickness(0, 0, 0, 14) });

        _body.FontSize = 15; _body.LineHeight = 24; _body.TextWrapping = TextWrapping.Wrap;
        root.Children.Add(_body);
        Closed += (_, _) =>
        {
            _closed = true;
            _results.ItemsSource = null; _body.Text = "";
            _shownResult = null; _shownDocument = null;
        };
    }

    private readonly Style _buttonStyle;

    private bool _acrylic;

    // Turunkan warna tema notch menjadi gaya kaca; dipanggil lagi saat tema berganti selama jendela terbuka.
    // Kunci yang sama dipakai gaya tombol/dropdown notch, jadi cukup mengganti isinya untuk jendela ini.
    public void UseTheme(ResourceDictionary source)
    {
        Color Of(string key) => ((SolidColorBrush)source[key]).Color;
        static SolidColorBrush Tint(Color color, byte alpha)
        {
            var brush = new SolidColorBrush(Color.FromArgb(alpha, color.R, color.G, color.B));
            brush.Freeze();
            return brush;
        }
        var soft = Of("NotchSoft");
        Resources["NotchBackground"] = Tint(Of("NotchBackground"), _acrylic ? (byte)0xB3 : (byte)0xFF);
        Resources["NotchPopup"] = Tint(Of("NotchBackground"), 0xF2); // Daftar dropdown tetap terbaca di atas desktop.
        Resources["NotchSoft"] = Tint(soft, 0x33);                   // Isi kaca.
        Resources["NotchSurface"] = Tint(Of("NotchAccent"), 0x59);   // Tepi kaca.
        Resources["NotchAccent"] = Tint(soft, 0x59);                 // Hover.
        Resources["NotchInk"] = Brushes.White;
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);
        var handle = new WindowInteropHelper(this).Handle;
        var on = 1;
        DwmSetWindowAttribute(handle, 20, ref on, sizeof(int)); // DWMWA_USE_IMMERSIVE_DARK_MODE: title bar gelap.
        // Render software (PC tanpa GPU) tidak menjamin piksel transparan: tetap latar solid.
        if (RenderCapability.Tier >> 16 == 0) return;
        var margins = new DwmMargins { Left = -1, Right = -1, Top = -1, Bottom = -1 };
        var acrylic = 3; // DWMSBT_TRANSIENTWINDOW (Windows 11 22H2+). Gagal di Windows lama: tetap solid.
        if (DwmExtendFrameIntoClientArea(handle, ref margins) != 0
            || DwmSetWindowAttribute(handle, 38, ref acrylic, sizeof(int)) != 0) return;
        if (HwndSource.FromHwnd(handle)?.CompositionTarget is { } target) target.BackgroundColor = Colors.Transparent;
        _acrylic = true;
        UseTheme(_main.Resources);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct DwmMargins { public int Left, Right, Top, Bottom; }
    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr window, int attribute, ref int value, int size);
    [DllImport("dwmapi.dll")]
    private static extern int DwmExtendFrameIntoClientArea(IntPtr window, ref DwmMargins margins);

    private void Search() => _main.SearchLyrics(_title.Text.Trim(), _artist.Text.Trim(), (LyricsMode)_mode.SelectedIndex);
    private Button Button(string text)
    {
        var button = new Button
        {
            Content = text, Style = _buttonStyle, Width = double.NaN, Height = 28, Padding = new Thickness(12, 0, 12, 0),
            Margin = new Thickness(0, 0, 6, 6), FontSize = 12
        };
        button.SetResourceReference(BorderBrushProperty, "NotchSurface"); // Tepi kaca lebih terang dari isinya.
        return button;
    }
    private static void Field(Control control)
    {
        control.SetResourceReference(BackgroundProperty, "NotchSoft");
        control.SetResourceReference(ForegroundProperty, "NotchInk");
        control.SetResourceReference(BorderBrushProperty, "NotchSurface");
        if (control is TextBox box) box.CaretBrush = Brushes.White;
        control.Padding = new Thickness(6, 4, 6, 4);
        control.VerticalContentAlignment = VerticalAlignment.Center;
    }
    private static TextBlock Label(string text) => new()
    { Text = text, FontWeight = FontWeights.SemiBold, Margin = new Thickness(0, 0, 0, 4) };
    public void ShowState(string title, string artist, LyricsMode mode, LyricsSearchResult? result,
        LyricCandidate? selected, LyricDocument? document, string status)
    {
        if (_closed) return;
        var key = title + "\u001F" + artist;
        if (_key != key)
        {
            var query = SongMatcher.CleanQuery(title, artist);
            _title.Text = query.Title; _artist.Text = query.Artist; _key = key;
        }
        _heading.Text = title.Length == 0 ? "Belum ada lagu" : $"{title} — {artist}";
        _mode.SelectedIndex = (int)mode; _status.Text = status;
        _save.Content = document?.IsSynced == true ? "Simpan .lrc" : "Simpan .txt";
        _save.IsEnabled = _main.CanExportLyrics; _load.IsEnabled = _main.CanImportLyrics;
        _saved.IsEnabled = _main.CanImportLyrics;
        _offsetText.Text = "Waktu lirik: " + _main.LyricsOffsetSeconds.ToString("+0.0;-0.0;0.0", CultureInfo.GetCultureInfo("id-ID")) + " dtk";
        _setting = true;
        try
        {
            if (!ReferenceEquals(_shownResult, result))
            {
                _results.ItemsSource = result?.Choices;
                _shownResult = result;
            }
            _results.SelectedItem = selected;
        }
        finally { _setting = false; }
        // Menyusun teks lengkap hanya ketika dokumen berubah dan jendela ini terbuka.
        if (!ReferenceEquals(_shownDocument, document) || _body.Text.Length == 0)
        {
            _body.Text = document?.PlainText ?? "Pilih hasil yang cocok untuk menampilkan teks di notch.";
            _shownDocument = document;
        }
    }
}

internal sealed class LrcRecord
{
    public long Id { get; set; }
    public string TrackName { get; set; } = "";
    public string ArtistName { get; set; } = "";
    public string AlbumName { get; set; } = "";
    public double Duration { get; set; }
    public bool Instrumental { get; set; }
    public string? PlainLyrics { get; set; }
    public string? SyncedLyrics { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string Source { get; set; } = "LRCLIB";
    [System.Text.Json.Serialization.JsonIgnore]
    public bool IsRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool MachineRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool IsLocal { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string? CacheFileName { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string Language { get; set; } = "";
    [System.Text.Json.Serialization.JsonIgnore]
    public double? ImportedOffsetSeconds { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public LyricLine[]? ParsedLines { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool HasTiming => ParsedLines?.Length > 0 || !string.IsNullOrWhiteSpace(SyncedLyrics);
}

internal sealed class LrcServiceException(string message) : Exception(message) { }

internal static class LrcClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };
    private static DateTimeOffset _nextRequestUtc;
    private const int MaxRetries = 3;
    private static readonly TimeSpan MaxAutomaticWait = TimeSpan.FromSeconds(8);
    private const string RomajiMarker = @"\b(romaji|romanized|romanised|romanization|romanisation)\b";

    public static async Task<List<LrcRecord>> FindAsync(
        string title, string artist, string album, double duration, CancellationToken token,
        IProgress<string>? progress = null)
    {
        if (!string.IsNullOrWhiteSpace(artist))
        {
            var path = $"api/get?track_name={Uri.EscapeDataString(title)}&artist_name={Uri.EscapeDataString(artist)}";
            if (!string.IsNullOrWhiteSpace(album)) path += "&album_name=" + Uri.EscapeDataString(album);
            if (duration >= 1 && duration <= 3600)
                path += "&duration=" + duration.ToString("0.###", CultureInfo.InvariantCulture);
            using var json = await RequestAsync(path, token, progress);
            if (json?.RootElement.ValueKind == JsonValueKind.Object)
            {
                var exact = json.RootElement.Deserialize<LrcRecord>(JsonOptions);
                if (exact is not null && CompactRecord(exact)) return [exact];
            }
        }
        var matches = await SearchAsync(title, artist, token, progress);
        if (matches.Count == 0)
        {
            using var json = await RequestAsync("api/search?q=" + Uri.EscapeDataString(title), token, progress);
            matches = ParseRecords(json);
        }
        return matches;
    }

    private static async Task<List<LrcRecord>> SearchAsync(
        string title, string artist, CancellationToken token, IProgress<string>? progress)
    {
        var path = "api/search?track_name=" + Uri.EscapeDataString(title);
        if (!string.IsNullOrWhiteSpace(artist)) path += "&artist_name=" + Uri.EscapeDataString(artist);
        using var json = await RequestAsync(path, token, progress);
        return ParseRecords(json);
    }

    public static async Task<List<LrcRecord>> FindRomajiAsync(
        string title, string artist, List<LrcRecord> originals, CancellationToken token,
        IProgress<string>? progress = null)
    {
        var candidates = originals.Where(IsRomaji).ToList();
        if (candidates.Count == 0)
            candidates = (await SearchAsync(title, artist, token, progress)).Where(IsRomaji).ToList();
        if (candidates.Count == 0)
        {
            var query = Uri.EscapeDataString($"{title} {artist} romaji");
            using var json = await RequestAsync("api/search?q=" + query, token, progress);
            candidates = ParseRecords(json).Where(IsRomaji).ToList();
        }
        return candidates.DistinctBy(r => r.Id).Take(20).ToList();
    }

    private static List<LrcRecord> ParseRecords(JsonDocument? document)
    {
        if (document?.RootElement.ValueKind != JsonValueKind.Array) return [];
        var records = new List<LrcRecord>(20);
        // Jangan membuat seluruh daftar objek lalu membuang mayoritas hasilnya.
        foreach (var item in document.RootElement.EnumerateArray().Take(20))
        {
            if (item.ValueKind != JsonValueKind.Object) continue;
            var record = item.Deserialize<LrcRecord>(JsonOptions);
            if (record is not null && CompactRecord(record)) records.Add(record);
        }
        return records;
    }

    private static bool CompactRecord(LrcRecord record)
    {
        if (record.SyncedLyrics?.Length > LyricLimits.SourceCharacters
            || record.PlainLyrics?.Length > LyricLimits.SourceCharacters) return false;
        record.TrackName = (record.TrackName ?? "")[..Math.Min(record.TrackName?.Length ?? 0, 512)];
        record.ArtistName = (record.ArtistName ?? "")[..Math.Min(record.ArtistName?.Length ?? 0, 512)];
        record.AlbumName = (record.AlbumName ?? "")[..Math.Min(record.AlbumName?.Length ?? 0, 512)];
        // Tolak sumber berlebihan; jangan menampilkan potongan lirik seolah-olah lengkap.
        // LRC sudah memuat teks, jadi salinan plain tidak perlu ditahan.
        if (!string.IsNullOrWhiteSpace(record.SyncedLyrics)
            && Regex.IsMatch(record.SyncedLyrics, @"\[\d{1,3}:[0-5]\d(?:[.:]\d{1,3})?\]"))
            record.PlainLyrics = null;
        return true;
    }

    public static bool HasKana(string text)
        => Regex.IsMatch(text, @"[\u3041-\u3096\u30A1-\u30FA\uFF66-\uFF9D]");

    public static bool IsMarkedRomaji(LrcRecord record)
        => record.IsRomanized || Regex.IsMatch(record.TrackName + " " + record.AlbumName, RomajiMarker, RegexOptions.IgnoreCase);

    private static bool IsRomaji(LrcRecord record)
    {
        var text = record.PlainLyrics ?? record.SyncedLyrics ?? "";
        // Tidak perlu membentuk salinan teks tanpa timestamp hanya untuk memeriksa aksara.
        // Teks Latin saja bisa juga terjemahan Inggris; perlu label romaji eksplisit.
        return IsMarkedRomaji(record) && Regex.IsMatch(text, "[A-Za-z]")
            && !Regex.IsMatch(record.TrackName + " " + record.AlbumName,
                @"\b(translation|translated|terjemahan)\b", RegexOptions.IgnoreCase)
            && !Regex.IsMatch(text, @"[\u3040-\u30FF\u3400-\u9FFF\uFF66-\uFF9D]");
    }

    public static string GetText(LrcRecord record)
    {
        if (!string.IsNullOrWhiteSpace(record.PlainLyrics)) return record.PlainLyrics.Trim();
        var text = record.SyncedLyrics ?? "";
        text = Regex.Replace(text, @"(?m)^\s*(?:\[\d+:\d+(?:[.:]\d+)?\])+\s*", "");
        text = Regex.Replace(text, @"<\d{1,3}:\d{2}(?:[.:]\d{1,3})?>", "");
        return Regex.Replace(text, @"(?m)^\[(?:ar|al|ti|by|offset):[^\]]*\]\s*$", "").Trim();
    }

    private static string Normalize(string text)
    {
        text = Regex.Replace(text ?? "", RomajiMarker, "", RegexOptions.IgnoreCase);
        return new string(text.Normalize(NormalizationForm.FormKC)
            .Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());
    }

    public static bool SameSong(LrcRecord record, string title, string artist, double duration)
        => !string.IsNullOrWhiteSpace(artist)
           && Normalize(record.TrackName) == Normalize(title)
           && Normalize(record.ArtistName) == Normalize(artist)
           && (duration <= 0 || Math.Abs(record.Duration - duration) <= 3);

    private static async Task<JsonDocument?> RequestAsync(
        string path, CancellationToken token, IProgress<string>? progress)
    {
        await Gate.WaitAsync(token);
        try
        {
            // Satu permintaan awal + paling banyak tiga percobaan ulang.
            // Tidak ada polling setelah batas ini atau setelah jendela ditutup.
            for (var attempt = 0; attempt <= MaxRetries; attempt++)
            {
                token.ThrowIfCancellationRequested();
                var wait = _nextRequestUtc - DateTimeOffset.UtcNow;
                if (wait > MaxAutomaticWait)
                    throw new LrcServiceException($"Layanan masih meminta jeda. Tunggu minimal {Math.Ceiling(wait.TotalSeconds)} detik sebelum Cari ulang, atau pilih Cari di browser.");
                if (wait > TimeSpan.Zero)
                {
                    if (attempt == 0 && wait.TotalSeconds > 0.5)
                        progress?.Report($"Menunggu jeda layanan {Math.Ceiling(wait.TotalSeconds)} detik…");
                    await Task.Delay(wait, token);
                }

                progress?.Report(attempt == 0 ? "Mencari lirik di LRCLIB…"
                    : $"Mencoba kembali ({attempt}/{MaxRetries})…");
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
                timeout.CancelAfter(TimeSpan.FromSeconds(15));
                using var response = await LyricHttp.Client.GetAsync("https://lrclib.net/" + path,
                    HttpCompletionOption.ResponseHeadersRead, timeout.Token);
                if (response.StatusCode == HttpStatusCode.TooManyRequests ||
                    response.StatusCode == HttpStatusCode.ServiceUnavailable)
                {
                    var rateLimited = response.StatusCode == HttpStatusCode.TooManyRequests;
                    var retry = response.Headers.RetryAfter;
                    var backoff = TimeSpan.FromSeconds(1 << attempt);
                    var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date
                        ? date - DateTimeOffset.UtcNow
                        : rateLimited ? TimeSpan.FromSeconds(30) : backoff);
                    // Hormati Retry-After, dengan jeda minimal yang meningkat: 1, 2, 4 detik.
                    // Jeda panjang tidak dipotong agar tidak melanggar batas layanan.
                    if (delay < backoff) delay = backoff;
                    try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                    catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                    var reason = rateLimited ? "Pencarian sementara dibatasi layanan."
                        : "Layanan lirik sedang sibuk.";
                    if (attempt == MaxRetries || delay > MaxAutomaticWait)
                    {
                        var exhausted = attempt == MaxRetries ? $"Sudah dicoba ulang {MaxRetries} kali. " : "";
                        throw new LrcServiceException($"{reason} {exhausted}Tunggu minimal {Math.Ceiling(delay.TotalSeconds)} detik sebelum Cari ulang, atau pilih Cari di browser.");
                    }
                    progress?.Report($"{reason} Mencoba lagi otomatis dalam {Math.Ceiling(delay.TotalSeconds)} detik ({attempt + 1}/{MaxRetries})…");
                    continue; // Respons dilepas sebelum jeda percobaan berikutnya.
                }
                if (response.StatusCode == HttpStatusCode.NotFound) return null;
                response.EnsureSuccessStatusCode();
                return await LyricHttp.ReadJsonAsync(response, timeout.Token);
            }
            throw new LrcServiceException("Lirik belum bisa dimuat. Coba Cari di browser.");
        }
        finally
        {
            var earliest = DateTimeOffset.UtcNow.AddMilliseconds(300);
            if (_nextRequestUtc < earliest) _nextRequestUtc = earliest;
            Gate.Release();
        }
    }
}

// Semua hasil dan teks dibatasi; tidak ada model AI, WebView, atau unduhan katalog lagu.
internal enum LyricsMode { Auto, Original, Romaji }
internal static class LyricsLanguage
{
    public static bool PrefersRomaji(LyricsMode mode, LrcRecord record)
        => mode != LyricsMode.Original && !record.Instrumental
            && (LrcClient.IsMarkedRomaji(record) || UnisonClient.IsJapanese(record));
}
internal sealed record LyricLine(double Seconds, string Text);
internal sealed record LyricCandidate(LrcRecord Record, double Score, bool Confident)
{
    public string DisplayName => Record.CacheFileName is not null
        ? $"Tersimpan · {Record.TrackName} — {Record.ArtistName}"
            + (Record.Duration > 0 ? $" · {(int)(Record.Duration / 60)}:{(int)Record.Duration % 60:00}" : "")
            + (Record.IsRomanized ? " · romaji" : "")
        : Record.Source == "Better Lyrics"
        ? $"Better Lyrics · romaji berwaktu · {Record.TrackName} — {Record.ArtistName}" + (Confident ? "" : " · periksa hasil")
        : (Record.IsLocal ? "Lokal · " : $"{Record.Source} · {Score:P0} mirip · ")
            + $"{Record.TrackName} — {Record.ArtistName} · " + (Record.HasTiming ? "berwaktu" : "teks biasa")
            + (Record.MachineRomanized ? " · romaji hasil konversi" : "");
}
internal sealed record LyricsSearchResult(List<LyricCandidate> Choices,
    bool WantRomaji, string Note);

internal sealed class LyricDocument
{
    public LyricLine[] Lines { get; private init; } = [];
    private string _plainText = "";
    // Hanya jendela Aa yang membutuhkan satu string berisi seluruh lagu.
    public string PlainText => IsSynced ? string.Join(Environment.NewLine, Lines.Select(x => x.Text)) : _plainText;
    public bool IsSynced => Lines.Length > 0;
    public bool Instrumental { get; private init; }
    public string[] Preview { get; private init; } = [];
    private static readonly Regex Stamp = new(@"\[(?<m>\d{1,4}):(?<s>\d{2})(?:[.:](?<f>\d{1,3}))?\]", RegexOptions.CultureInvariant);

    public static LyricDocument From(LrcRecord record)
    {
        if (record.ParsedLines is { Length: > 0 } parsed) return Synced(parsed, record.Instrumental);
        var text = record.SyncedLyrics ?? "";
        if (text.Length > LyricLimits.TextFileCharacters) return new();
        var offsetMatch = Regex.Match(text, @"\[offset:([+-]?\d{1,8})\]", RegexOptions.IgnoreCase);
        var offset = offsetMatch.Success && double.TryParse(offsetMatch.Groups[1].Value,
            NumberStyles.Integer, CultureInfo.InvariantCulture, out var ms) ? ms / 1000 : 0;
        var lines = new List<LyricLine>();
        using var reader = new StringReader(text);
        var rawCount = 0;
        var parsedCharacters = 0;
        while (reader.ReadLine() is string raw)
        {
            if (++rawCount > LyricLimits.Lines + 32) return new();
            var stamps = Stamp.Matches(raw);
            if (stamps.Count == 0) continue;
            if (stamps.Count > 8) return new();
            var words = Stamp.Replace(raw, "").Trim();
            words = Regex.Replace(words, @"<\d{1,3}:\d{2}(?:[.:]\d{1,3})?>", "");
            if (words.Length > 4096) return new();
            foreach (Match stamp in stamps)
            {
                var seconds = int.Parse(stamp.Groups["s"].Value, CultureInfo.InvariantCulture);
                if (seconds >= 60) continue;
                var fraction = stamp.Groups["f"].Value;
                var time = int.Parse(stamp.Groups["m"].Value, CultureInfo.InvariantCulture) * 60 + seconds;
                var part = fraction.Length == 0 ? 0 : int.Parse(fraction, CultureInfo.InvariantCulture) / Math.Pow(10, fraction.Length);
                if (time + part - offset > 86400) return new();
                parsedCharacters += words.Length + 3;
                if (parsedCharacters > LyricLimits.SourceCharacters) return new();
                lines.Add(new LyricLine(Math.Max(0, time + part - offset), words));
                if (lines.Count > LyricLimits.Lines) return new();
            }
        }
        if (lines.Count > 0)
        {
            var sorted = lines.GroupBy(x => x.Seconds).OrderBy(g => g.Key)
                .Select(g => new LyricLine(g.Key, string.Join(" / ", g.Select(x => x.Text).Distinct()))).ToArray();
            if (sorted.Any(x => x.Text.Length > 4096)) return new();
            record.ParsedLines = sorted;
            record.SyncedLyrics = null; record.PlainLyrics = null;
            return Synced(sorted, record.Instrumental);
        }
        var plain = record.Instrumental ? "Instrumental — tidak ada lirik." : LrcClient.GetText(record);
        if (plain.Length > LyricLimits.SourceCharacters) return new();
        var preview = new List<string>(2);
        using var plainReader = new StringReader(plain);
        while (preview.Count < 2 && plainReader.ReadLine() is string row)
            if (!string.IsNullOrWhiteSpace(row)) preview.Add(row.Trim());
        return new LyricDocument { _plainText = plain, Instrumental = record.Instrumental, Preview = preview.ToArray() };
    }

    private static LyricDocument Synced(LyricLine[] lines, bool instrumental)
        => new() { Lines = lines, Instrumental = instrumental,
            Preview = lines.Where(x => !string.IsNullOrWhiteSpace(x.Text)).Take(2).Select(x => x.Text).ToArray() };

    public int IndexAt(double seconds)
    {
        var low = 0;
        var high = Lines.Length - 1;
        while (low <= high)
        {
            var mid = low + (high - low) / 2;
            if (Lines[mid].Seconds <= seconds) low = mid + 1;
            else high = mid - 1;
        }
        return high;
    }
}

internal static class SongMatcher
{
    private const string Roman = @"\b(?:romaji|romanized|romanised|romanization|romanisation)\b";
    private const string Video = @"\b(?:official\s+(?:music\s+)?(?:video|audio|lyric\s+video|mv)|music\s+video|lyric\s+video|lyrics\s+video|visuali[sz]er|official\s+mv|hd|4k)\b";
    public static bool IsRomajiLabel(string text) => Regex.IsMatch(text, Roman, RegexOptions.IgnoreCase);
    public static bool IsTranslation(string text) => Regex.IsMatch(text,
        @"\b(?:english|indonesian|spanish|french|german|portuguese|russian|korean|japanese|chinese)\s+translation\b|\bterjemahan\b|Genius\s+(?!Romanizations)\w*\s*Translations", RegexOptions.IgnoreCase);
    private static string Limit(string value, int max = 192) => value.Length > max ? value[..max] : value;
    public static string CleanTitle(string text)
    {
        text = Limit(text ?? "", 512).Replace('\u200B', ' ');
        text = Regex.Replace(text, Video, "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, Roman, "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, @"(?:\s+|[\(\[【])(?:M/?V|PV)(?:[\)\]】])?\s*$", "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, @"[\(\[【]\s*[\)\]】]", "");
        return Regex.Replace(text, @"\s+", " ").Trim(' ', '-', '–', '|');
    }
    public static string CleanArtist(string text) => Regex.Replace(Limit(text ?? "", 256),
        @"\s*(?:-\s*Topic|Official(?:\s+Channel)?)\s*$", "", RegexOptions.IgnoreCase).Trim();
    public static (string Title, string Artist) CleanQuery(string title, string artist)
    {
        title = CleanTitle(title);
        artist = CleanArtist(artist);
        // Format YouTube seperti Artis『Judul』Official Music Video.
        var framed = Regex.Match(title, @"^(?<artist>[^『「]{1,128})[『「](?<title>[^』」]{1,512})[』」]\s*$");
        if (framed.Success)
        {
            var lead = CleanArtist(framed.Groups["artist"].Value);
            var ageSuffix = artist.StartsWith(lead, StringComparison.OrdinalIgnoreCase)
                && Regex.IsMatch(artist[lead.Length..], @"^\s*\(\d{1,2}\)\s*$");
            if (Similarity(lead, artist) >= 0.95 || ageSuffix || artist.Length == 0)
            { artist = lead; title = framed.Groups["title"].Value.Trim(); }
        }
        var split = Regex.Match(title, @"^(?<left>.+?)\s+[-–—]\s+(?<right>.+)$");
        if (split.Success)
        {
            var left = split.Groups["left"].Value.Trim();
            var right = split.Groups["right"].Value.Trim();
            if (Similarity(CleanArtist(left), artist) >= 0.95)
                title = right;
            else if (Similarity(CleanArtist(right), artist) >= 0.95)
                title = left; // Contoh: Heart Forecast - Eve MV, artis Eve.
            else if (string.IsNullOrWhiteSpace(artist))
            { artist = CleanArtist(left); title = right; }
        }
        return (title, artist);
    }
    private static string Normalize(string text)
    {
        text = Limit(text).Normalize(NormalizationForm.FormKC).Normalize(NormalizationForm.FormD);
        var chars = text.Where(c => CharUnicodeInfo.GetUnicodeCategory(c) != UnicodeCategory.NonSpacingMark)
            .Select(c => char.IsLetterOrDigit(c) ? char.ToLowerInvariant(c) : ' ').ToArray();
        return Limit(Regex.Replace(new string(chars), @"\s+", " ").Trim());
    }
    public static double Similarity(string first, string second)
    {
        var a = Normalize(first);
        var b = Normalize(second);
        if (a.Length == 0 || b.Length == 0) return 0;
        if (a == b) return 1;
        if (a.Length < b.Length) (a, b) = (b, a);
        // One bounded row, at most 193 integers. No quadratic matrix allocation.
        Span<int> row = stackalloc int[b.Length + 1];
        for (var j = 0; j <= b.Length; j++) row[j] = j;
        for (var i = 1; i <= a.Length; i++)
        {
            var diagonal = row[0]; row[0] = i;
            for (var j = 1; j <= b.Length; j++)
            {
                var old = row[j];
                row[j] = Math.Min(Math.Min(row[j] + 1, row[j - 1] + 1), diagonal + (a[i - 1] == b[j - 1] ? 0 : 1));
                diagonal = old;
            }
        }
        var edit = 1.0 - (double)row[b.Length] / a.Length;
        var left = a.Split(' ').ToHashSet();
        var right = b.Split(' ').ToHashSet();
        var dice = 2.0 * left.Intersect(right).Count() / (left.Count + right.Count);
        return Math.Max(edit, dice * 0.96);
    }
    public static string[] Aliases(string title, string artist)
    {
        title = CleanQuery(title, artist).Title;
        var aliases = new List<string> { title };
        foreach (Match match in Regex.Matches(title, @"[\(（]([^\)）]+)[\)）]").Cast<Match>().Take(4))
        {
            var alias = match.Groups[1].Value.Trim();
            if (Regex.IsMatch(title, @"[\u3040-\u30FF\u3400-\u9FFF]") &&
                !Regex.IsMatch(alias, @"\b(?:live|remix|cover|version|ver|acoustic|instrumental|feat|ft)\b", RegexOptions.IgnoreCase))
                aliases.Add(alias);
        }
        var noAlias = Regex.Replace(title, @"[\(（][^\)）]+[\)）]", "").Trim();
        // Version labels are meaningful and must remain part of matching.
        if (!Regex.IsMatch(title, @"\b(?:live|remix|cover|acoustic|instrumental|version|ver)\b", RegexOptions.IgnoreCase)) aliases.Add(noAlias);
        return aliases.Where(x => !string.IsNullOrWhiteSpace(x)).Distinct().Take(6).ToArray();
    }
    public static double TitleScore(string title, string artist, string query)
        => Aliases(title, artist).Select(x => Similarity(x, query)).DefaultIfEmpty(0).Max();
    public static List<LyricCandidate> Rank(IEnumerable<LrcRecord> records, string title, string artist, double duration,
        IReadOnlyList<string>? queryAliases = null)
    {
        return records.Take(20).Select(r =>
        {
            var ts = (queryAliases ?? new[] { title }).Take(6).Select(q => TitleScore(r.TrackName, r.ArtistName, q)).DefaultIfEmpty(0).Max();
            var ars = Similarity(CleanArtist(r.ArtistName), artist);
            var score = string.IsNullOrWhiteSpace(artist) ? ts * 0.85 : 0.75 * ts + 0.25 * ars;
            var durationFits = duration <= 0 || (r.Duration > 0 && Math.Abs(r.Duration - duration) <= 3);
            if (!durationFits) score *= 0.8;
            return new LyricCandidate(r, score, ts >= 0.90 && ars >= 0.82 && score >= 0.90 && durationFits);
        }).Where(c => c.Score >= 0.40 || (LrcClient.IsMarkedRomaji(c.Record) && Similarity(c.Record.ArtistName, artist) >= 0.85))
            .OrderByDescending(c => c.Score).ThenByDescending(c => c.Record.HasTiming).ToList();
    }
    public static LyricCandidate? AutoChoice(List<LyricCandidate> choices)
    {
        var first = choices.FirstOrDefault();
        if (first is null || !first.Confident) return null;
        var next = choices.Skip(1).FirstOrDefault(x => Normalize(x.Record.TrackName) != Normalize(first.Record.TrackName)
            || Normalize(x.Record.ArtistName) != Normalize(first.Record.ArtistName)
            || Math.Abs(x.Record.Duration - first.Record.Duration) > 3);
        return next is null || first.Score - next.Score >= 0.06 ? first : null;
    }
}

internal static class LyricsSearch
{
    public static async Task<LyricsSearchResult> FindAsync(string title, string artist, string album, double duration,
        LyricsMode mode, CancellationToken token, IProgress<string>? progress)
    {
        var rawTitle = title; var rawArtist = artist; var japaneseHint = false;
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        album = album[..Math.Min(album.Length, 512)];
        var wantRomaji = false;
        var records = new List<LrcRecord>();
        var notes = new List<string>();
        var betterQueries = new HashSet<string>(StringComparer.Ordinal);
        var unisonQueries = new HashSet<string>(StringComparer.Ordinal);
        void AddNote(Exception ex, string provider)
            => notes.Add(ex is LrcServiceException ? ex.Message : provider + " belum dapat dihubungi.");
        string Notes() => string.Join(" ", notes.Distinct().Take(5));
        List<LyricCandidate> Rank(IEnumerable<LrcRecord> input) => SongMatcher.Rank(input
            .Where(x => !SongMatcher.IsTranslation(x.TrackName + " " + x.AlbumName))
            .DistinctBy(x => (x.Source, x.Id))
            .OrderByDescending(x => LrcClient.IsMarkedRomaji(x) && x.HasTiming), title, artist, duration);
        void DetectJapanese()
        {
            var first = SongMatcher.AutoChoice(Rank(records));
            wantRomaji = first is not null ? LyricsLanguage.PrefersRomaji(mode, first.Record) : japaneseHint;
        }
        LyricsSearchResult Result()
        {
            DetectJapanese();
            // Romaji adalah pilihan penyajian Jepang, bukan filter yang membuang bahasa lain.
            var eligible = mode == LyricsMode.Original ? records.Where(x => !LrcClient.IsMarkedRomaji(x)) : records;
            var ranked = Rank(eligible);
            var automatic = SongMatcher.AutoChoice(ranked);
            var retained = LyricLimits.Retain(ranked);
            if (automatic is null && retained.Count > 0) retained[0] = retained[0] with { Confident = false };
            return new(retained, wantRomaji, Notes());
        }
        bool Ready(LyricsSearchResult result)
        {
            var selected = SongMatcher.AutoChoice(result.Choices);
            return selected is not null && (selected.Record.Instrumental || selected.Record.HasTiming);
        }
        async Task<LyricCandidate?> TryBetter(string t, string a, string al, double d)
        {
            var key = $"{t}\u001f{a}\u001f{al}\u001f{Math.Round(d)}";
            if (betterQueries.Count >= 2 || !betterQueries.Add(key)) return null;
            token.ThrowIfCancellationRequested();
            progress?.Report("Mencari romaji berwaktu di Better Lyrics…");
            var result = await BetterLyricsClient.FindRomajiAsync(t, a, al, d, token);
            if (result.Candidate is null) notes.Add(result.Note);
            return result.Candidate;
        }
        async Task TryUnison(string t, string a, string al, double d)
        {
            var key = $"{t}\u001f{a}\u001f{al}\u001f{Math.Round(d)}";
            if (unisonQueries.Count >= 2 || !unisonQueries.Add(key)) return;
            progress?.Report("Mencari teks dan penanda waktu di Unison…");
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(12));
            try
            {
                var extra = await UnisonClient.FindAsync(t, a, al, d, mode != LyricsMode.Original, budget.Token);
                records = extra.Concat(Rank(records).Select(x => x.Record))
                    .DistinctBy(x => (x.Source, x.Id)).Take(20).ToList();
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Unison"); }
        }
        LyricsSearchResult FromBetter(LyricCandidate candidate)
            => new([candidate], true, "Romaji berwaktu dari Better Lyrics. " + Notes());

        // Video YouTube tidak punya album dan judulnya sering berbeda dari judul rilis.
        // Kenali lagu aslinya dulu agar LRCLIB menerima judul, artis, dan durasi audio yang benar.
        if (album.Length == 0)
        {
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(8));
            try
            {
                progress?.Report("Mengenali lagu asli dari judul video…");
                var song = await SongIdentifier.FindAsync(rawTitle, rawArtist, duration, budget.Token);
                if (song is not null)
                {
                    // Album katalog bisa berbeda dari LRCLIB; judul, artis, dan durasi cukup.
                    title = song.Title; artist = song.Artist;
                    if (song.Duration > 0) duration = song.Duration;
                }
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Katalog lagu"); }
        }
        japaneseHint = mode == LyricsMode.Romaji || (mode == LyricsMode.Auto && LrcClient.HasKana(title));

        progress?.Report("Mencari teks dan penanda waktu di LRCLIB…");
        using (var budget = CancellationTokenSource.CreateLinkedTokenSource(token))
        {
            budget.CancelAfter(TimeSpan.FromSeconds(16));
            try { records.AddRange(await LrcClient.FindAsync(title, artist, album, duration, budget.Token, progress)); }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "LRCLIB"); }
        }
        // LRCLIB dan Unison mencari lirik semua bahasa terlebih dahulu, termasuk teks tanpa waktu.
        if (!Ready(Result())) await TryUnison(title, artist, album, duration);
        DetectJapanese();
        var original = SongMatcher.AutoChoice(Rank(records));
        if (wantRomaji && original?.Record.Instrumental != true
            && (original is null || !LrcClient.IsMarkedRomaji(original.Record)))
        {
            var canonical = original?.Record;
            var better = await TryBetter(canonical?.TrackName ?? title, canonical?.ArtistName ?? artist,
                string.IsNullOrWhiteSpace(canonical?.AlbumName) ? album : canonical!.AlbumName,
                canonical?.Duration > 0 ? canonical.Duration : duration);
            if (better is { Confident: true } || (better is not null && records.Count == 0)) return FromBetter(better!);
            if (better is not null) notes.Add("Kecocokan romaji belum kuat; lirik asli tetap menjadi pilihan.");
            await TryUnison(canonical?.TrackName ?? title, canonical?.ArtistName ?? artist,
                string.IsNullOrWhiteSpace(canonical?.AlbumName) ? album : canonical!.AlbumName,
                canonical?.Duration > 0 ? canonical.Duration : duration);
            var found = Result();
            if (Ready(found)) return found; // Baris Jepang yang cocok dikonversi setelah dipilih.
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(8));
            try { records.AddRange(await LrcClient.FindRomajiAsync(title, artist, records, budget.Token, progress)); }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Romaji LRCLIB"); }
        }
        token.ThrowIfCancellationRequested();
        return Result();
    }
}

internal sealed record IdentifiedSong(string Title, string Artist, string Album, double Duration);

// Mencocokkan judul video bebas dengan katalog Deezer (publik, tanpa token).
// Hasil hanya dipakai bila judul dan artis/durasi sama-sama cocok; selain itu pencarian lama berjalan.
internal static class SongIdentifier
{
    private const RegexOptions Flags = RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;
    private const string LatinNoise = @"\b(?:official|music\s*video|video|audio|lyrics?|lirik|mv|pv|4k|hd|\d{3,4}p|sub(?:title)?s?|terjemahan|romaji|eng|english|indo|full\s*ver(?:sion)?|ver\.?|version|cover|live|remaster(?:ed)?|visuali[sz]er|op|ed|opening|ending|ost|anime|nightcore|slowed|reverb|sped\s*up|8d|karaoke|tiktok|color\s*coded)\b";
    private const string CjkNoise = "歌詞|字幕|歌ってみた|弾いてみた|叩いてみた|公式|主題歌|オープニング|エンディング|カラオケ|フル";
    private const string Separator = @"\s[-–—~]\s|[|｜／]|\s/\s";
    private const string Version = @"(?:live|remix|acoustic|instrumental|karaoke|sped\s*up|slowed|remaster(?:ed)?|tour|unplugged)";

    static SongIdentifier() => SelfCheck();

    private static string Key(string text) => new((text ?? "").Normalize(NormalizationForm.FormKC)
        .Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());

    private static string Artist(string artist)
        => Regex.Replace(SongMatcher.CleanArtist(artist), @"\s*VEVO$", "", Flags).Trim();

    // Judul bersih (pemisah tetap ada), bagian dalam 『』/「」, dan potongan per pemisah.
    public static (string Whole, string Quoted, string[] Parts) Clean(string rawTitle)
    {
        var text = (rawTitle ?? "")[..Math.Min(rawTitle?.Length ?? 0, 512)];
        var quoted = Regex.Match(text, @"[『「]([^』」]{1,128})[』」]").Groups[1].Value.Trim();
        text = Regex.Replace(text, @"#\S+", " ");
        text = Regex.Replace(text, @"[\(\[【（［〔]([^\)\]】）］〕]{0,128})[\)\]】）］〕]", m =>
            Regex.IsMatch(m.Groups[1].Value, LatinNoise, Flags) || Regex.IsMatch(m.Groups[1].Value, CjkNoise) ? " " : m.Value);
        text = Regex.Replace(text, @"\s(?:feat\.?|ft\.?|prod\.?\s*by|prod\.)\s[^\(\[【|｜／-]+", " ", Flags);
        text = Regex.Replace(SongMatcher.CleanTitle(text), @"[『』「」""]", " ");
        text = Regex.Replace(text, @"\s+", " ").Trim(' ', '-', '–', '|', '/');
        var parts = Regex.Split(text, Separator).Select(x => x.Trim()).Where(x => x.Length > 0).ToArray();
        return (text, quoted, parts);
    }

    public static string[] Queries(string rawTitle, string rawArtist)
    {
        var (whole, quoted, parts) = Clean(rawTitle);
        var artist = Artist(rawArtist);
        var flat = Regex.Replace(whole, Separator, " ").Trim();
        var withArtist = artist.Length > 0 && !Key(flat).Contains(Key(artist)) ? flat + " " + artist : flat;
        var longest = parts.Where(x => Key(x) != Key(artist)).OrderByDescending(x => x.Length).FirstOrDefault() ?? "";
        return new[] { quoted.Length > 0 ? quoted + " " + artist : withArtist, flat, longest }
            .Select(x => x.Trim()).Where(x => x.Length > 0).Distinct(StringComparer.OrdinalIgnoreCase).Take(3).ToArray();
    }

    public static async Task<IdentifiedSong?> FindAsync(string rawTitle, string rawArtist, double duration, CancellationToken token)
    {
        var (whole, quoted, parts) = Clean(rawTitle);
        var artist = Artist(rawArtist);
        var context = Key(rawTitle + " " + artist);
        var titles = parts.Append(whole).Append(quoted).Where(x => x.Length > 0).ToArray();
        foreach (var query in Queries(rawTitle, rawArtist))
        {
            token.ThrowIfCancellationRequested();
            using var response = await LyricHttp.Client.GetAsync("https://api.deezer.com/search?limit=10&q=" + Uri.EscapeDataString(query),
                HttpCompletionOption.ResponseHeadersRead, token);
            if (!response.IsSuccessStatusCode) return null;
            using var json = await LyricHttp.ReadJsonAsync(response, token);
            if (!json.RootElement.TryGetProperty("data", out var data) || data.ValueKind != JsonValueKind.Array) continue;
            IdentifiedSong? best = null; var bestScore = 0.0;
            foreach (var item in data.EnumerateArray().Take(10))
            {
                var song = Read(item);
                if (song is null) continue;
                var key = Key(song.Title);
                var titleScore = titles.Select(x => SongMatcher.Similarity(song.Title, x)).DefaultIfEmpty(0).Max();
                if (key.Length >= 2 && context.Contains(key)) titleScore = Math.Max(titleScore, 0.92);
                var artistKey = Key(song.Artist);
                var artistHit = SongMatcher.Similarity(song.Artist, artist) >= 0.8 || (artistKey.Length >= 2 && context.Contains(artistKey));
                // Tanpa artis yang cocok, judul pendek seperti "Lemon" terlalu mudah tertukar dengan lagu lain.
                if (titleScore < 0.8 || !artistHit) continue;
                // MV boleh lebih panjang (intro/outro), tetapi rekaman yang lebih panjang biasanya versi lain.
                var longer = duration > 0 && song.Duration > 0 && (song.Duration - duration > 10 || duration - song.Duration > 90);
                var otherVersion = Regex.Matches(song.Title + " " + song.Album, Version, Flags).Any(m => !Regex.IsMatch(rawTitle, Regex.Escape(m.Value), Flags));
                var score = titleScore - (longer ? 0.2 : 0) - (otherVersion ? 0.3 : 0);
                if (score > bestScore) { best = song; bestScore = score; } // Seri: urutan relevansi Deezer menang.
            }
            if (best is not null) return best;
        }
        return null;
    }

    private static IdentifiedSong? Read(JsonElement item)
    {
        static string Text(JsonElement e, string name)
        {
            if (e.ValueKind != JsonValueKind.Object || !e.TryGetProperty(name, out var v) || v.ValueKind != JsonValueKind.String) return "";
            var value = v.GetString() ?? "";
            return value[..Math.Min(value.Length, 256)];
        }
        if (item.ValueKind != JsonValueKind.Object) return null;
        var title = Text(item, "title_short"); if (title.Length == 0) title = Text(item, "title");
        var artist = item.TryGetProperty("artist", out var a) ? Text(a, "name") : "";
        var album = item.TryGetProperty("album", out var al) ? Text(al, "title") : "";
        var seconds = item.TryGetProperty("duration", out var d) && d.ValueKind == JsonValueKind.Number
            && d.TryGetDouble(out var value) && value is > 0 and <= 3600 ? value : 0;
        return title.Length == 0 || artist.Length == 0 ? null : new(title, artist, album, seconds);
    }

    [Conditional("DEBUG")]
    private static void SelfCheck()
    {
        var q = Queries("YOASOBI「アイドル」Official Music Video", "Ayase / YOASOBI");
        Debug.Assert(q[0].StartsWith("アイドル"), string.Join(" | ", q));
        q = Queries("【MV】米津玄師 - Lemon (歌詞付き)", "米津玄師");
        Debug.Assert(q.All(x => !x.Contains("MV") && !x.Contains("歌詞")) && q[0].Contains("Lemon"), string.Join(" | ", q));
        q = Queries("Tulus - Hati-Hati di Jalan (Official Lyric Video) #tulus", "TULUS");
        Debug.Assert(q[0] == "Tulus Hati-Hati di Jalan" && q.Contains("Hati-Hati di Jalan"), string.Join(" | ", q));
        q = Queries("Ed Sheeran - Shape of You [Official Video]", "Ed SheeranVEVO");
        Debug.Assert(q[0] == "Ed Sheeran Shape of You", string.Join(" | ", q));
    }
}

// Penyedia lirik memakai satu HttpClient dan membaca JSON langsung dari stream.
internal static class LyricHttp
{
    public const int MaxJsonBytes = 2 * 1024 * 1024;
    public static readonly HttpClient Client = CreateClient();

    private static HttpClient CreateClient()
    {
        var handler = new SocketsHttpHandler
        {
            AllowAutoRedirect = false,
            UseCookies = false,
            AutomaticDecompression = DecompressionMethods.GZip | DecompressionMethods.Deflate,
            MaxConnectionsPerServer = 2,
            MaxResponseHeadersLength = 32,
            PooledConnectionIdleTimeout = TimeSpan.FromSeconds(45),
            PooledConnectionLifetime = TimeSpan.FromMinutes(5)
        };
        var client = new HttpClient(handler) { Timeout = System.Threading.Timeout.InfiniteTimeSpan };
        client.DefaultRequestHeaders.UserAgent.ParseAdd($"NotchMusic/{AppInfo.Version} (personal Windows desktop project)");
        client.DefaultRequestHeaders.Accept.ParseAdd("application/json");
        return client;
    }

    public static async Task<JsonDocument> ReadJsonAsync(HttpResponseMessage response, CancellationToken token)
    {
        if (response.Content.Headers.ContentLength is long size && size > MaxJsonBytes)
            throw new LrcServiceException("Respons lirik terlalu besar; pencarian dihentikan agar memori tetap terbatas.");
        if (!(response.Content.Headers.ContentType?.MediaType ?? "").Contains("json", StringComparison.OrdinalIgnoreCase))
            throw new LrcServiceException("Layanan mengirim format selain JSON.");
        await using var body = await response.Content.ReadAsStreamAsync(token);
        using var limited = new LimitedReadStream(body, MaxJsonBytes);
        return await JsonDocument.ParseAsync(limited, new JsonDocumentOptions { MaxDepth = 32 }, token);
    }

    // Content-Length dapat kosong atau merupakan ukuran terkompresi: hitung byte yang benar-benar dibaca.
    private sealed class LimitedReadStream(Stream inner, int limit) : Stream
    {
        private int _read;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => _read; set => throw new NotSupportedException(); }
        private int Allow(int count) => Math.Min(count, limit - _read + 1);
        private int Count(int count)
        {
            _read += count;
            if (_read > limit) throw new LrcServiceException("Respons lirik melewati batas ukuran.");
            return count;
        }
        public override int Read(byte[] buffer, int offset, int count)
            => Count(inner.Read(buffer, offset, Allow(count)));
        public override int Read(Span<byte> buffer) => Count(inner.Read(buffer[..Allow(buffer.Length)]));
        public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken token = default)
            => Count(await inner.ReadAsync(buffer[..Allow(buffer.Length)], token));
        public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken token)
            => ReadAsync(buffer.AsMemory(offset, count), token).AsTask();
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        // Pemilik respons yang menutup stream asli.
    }
}

internal static class LyricLimits
{
    public const int SourceCharacters = 64000;
    public const int CandidateCharacters = 128000;
    public const int Candidates = 8;
    public const int Lines = 2000;
    public const int TextFileCharacters = SourceCharacters + Lines * 32 + 4096;

    public static List<LyricCandidate> Retain(List<LyricCandidate> ranked)
    {
        var result = new List<LyricCandidate>(Candidates);
        var used = 0;
        foreach (var candidate in ranked)
        {
            var record = candidate.Record;
            var size = (record.PlainLyrics?.Length ?? 0) + (record.SyncedLyrics?.Length ?? 0)
                + (record.ParsedLines?.Sum(x => x.Text.Length) ?? 0);
            if (used + size > CandidateCharacters) continue;
            result.Add(candidate); used += size;
            if (result.Count == Candidates) break;
        }
        return result;
    }
}

internal sealed record BetterLyricsResult(LyricCandidate? Candidate, string Note);

internal static class BetterLyricsClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static DateTimeOffset _nextRequestUtc;
    // Satu hasil gagal saja, bukan cache katalog/riwayat lagu yang bertambah terus.
    private static string? _missKey;
    private static string _missNote = "";
    private static DateTimeOffset _missUntil;

    public static async Task<BetterLyricsResult> FindRomajiAsync(string title, string artist, string album,
        double duration, CancellationToken token)
    {
        if (string.IsNullOrWhiteSpace(title) || string.IsNullOrWhiteSpace(artist))
            return new(null, "Better Lyrics memerlukan judul dan artis.");
        album = album.Length > 512 ? album[..512] : album;
        var seconds = double.IsFinite(duration) && duration > 0
            ? (int)Math.Min(86400, Math.Round(duration, MidpointRounding.AwayFromZero)) : 0;
        var key = $"{title}\u001F{artist}\u001F{album}\u001F{seconds}";
        await Gate.WaitAsync(token);
        try
        {
            var now = DateTimeOffset.UtcNow;
            if (_missKey == key && now < _missUntil) return new(null, _missNote);
            if (now < _nextRequestUtc) return new(null, "Better Lyrics sedang meminta jeda; sumber lain dicoba.");
            BetterLyricsResult Miss(string note)
            {
                _missKey = key; _missNote = note; _missUntil = DateTimeOffset.UtcNow.AddSeconds(60);
                return new(null, note);
            }
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
            timeout.CancelAfter(TimeSpan.FromSeconds(8));
            var path = "https://api.betterlyrics.org/getLyrics?s=" + Uri.EscapeDataString(title)
                + "&a=" + Uri.EscapeDataString(artist) + "&al=" + Uri.EscapeDataString(album)
                + "&d=" + seconds.ToString(CultureInfo.InvariantCulture);
            using var response = await LyricHttp.Client.GetAsync(path, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            if (response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden)
                return Miss("Better Lyrics belum menyediakan hasil ini tanpa akses tambahan; sumber lain dicoba.");
            if (response.StatusCode is HttpStatusCode.TooManyRequests or HttpStatusCode.ServiceUnavailable)
            {
                var retry = response.Headers.RetryAfter;
                var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date
                    ? date - DateTimeOffset.UtcNow : TimeSpan.FromSeconds(60));
                if (delay < TimeSpan.FromSeconds(1)) delay = TimeSpan.FromSeconds(1);
                try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                return Miss("Better Lyrics sedang meminta jeda; sumber lain dicoba.");
            }
            if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.NoContent)
                return Miss("Romaji Better Lyrics belum tersedia; sumber lain dicoba.");
            if (!response.IsSuccessStatusCode)
                return Miss($"Better Lyrics belum dapat diakses (HTTP {(int)response.StatusCode}); sumber lain dicoba.");
            using var json = await LyricHttp.ReadJsonAsync(response, timeout.Token);
            if (json.RootElement.ValueKind != JsonValueKind.Object
                || !json.RootElement.TryGetProperty("ttml", out var value) || value.ValueKind != JsonValueKind.String)
                return Miss("Better Lyrics belum mengirim lirik berwaktu; sumber lain dicoba.");
            var ttml = value.GetString() ?? "";
            var lines = TtmlRomajiParser.Parse(ttml, duration, timeout.Token);
            if (lines.Length == 0)
                return Miss("Romaji berwaktu Better Lyrics belum lengkap atau belum cocok; sumber lain dicoba.");
            double? score = json.RootElement.TryGetProperty("score", out var match)
                && match.ValueKind == JsonValueKind.Number && match.TryGetDouble(out var number)
                && double.IsFinite(number) && number is >= 0 and <= 1 ? number : null;
            var record = new LrcRecord { Id = -1, TrackName = title, ArtistName = artist, AlbumName = album,
                Duration = duration, Source = "Better Lyrics", IsRomanized = true, ParsedLines = lines };
            // Cache hit tidak menyertakan score. Tidak tampilkan persentase kecocokan buatan.
            // API telah memilih satu lagu dari permintaan; score rendah tetap perlu pilihan manual.
            return new(new LyricCandidate(record, score ?? 1, score is null || score >= 0.90),
                "Romaji dan timing dari Better Lyrics. Periksa versi rekaman jika waktu atau teks berbeda.");
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
        catch (OperationCanceledException) { return new(null, "Better Lyrics melewati batas 8 detik; sumber lain dicoba."); }
        catch (Exception ex) when (ex is HttpRequestException or IOException or JsonException or System.Xml.XmlException or LrcServiceException)
        { return new(null, "Respons Better Lyrics belum bisa digunakan; sumber lain dicoba."); }
        finally { Gate.Release(); }
    }
}

// TTML: romaji ja-Latn memakai referensi baris asli; pembacaan asli dipakai untuk fallback Unison.
internal static class TtmlRomajiParser
{
    private const int MaxXmlCharacters = 384000;
    private const int MaxLineCharacters = 4096;
    private sealed record Entry(string Key, double? Begin, double? End, string Text);

    public static LyricLine[] Parse(string xml, double duration, CancellationToken token)
        => ParseCore(xml, duration, token, true);

    public static LyricLine[] ParseOriginal(string xml, double duration, CancellationToken token)
        => ParseCore(xml, duration, token, false);

    private static LyricLine[] ParseCore(string xml, double duration, CancellationToken token, bool requireRomaji)
    {
        if (xml.Length == 0 || xml.Length > MaxXmlCharacters) return [];
        var settings = new System.Xml.XmlReaderSettings { DtdProcessing = System.Xml.DtdProcessing.Prohibit,
            XmlResolver = null, IgnoreComments = true, MaxCharactersInDocument = MaxXmlCharacters };
        using var reader = System.Xml.XmlReader.Create(new StringReader(xml), settings);
        var romanized = new Dictionary<string, Entry>(StringComparer.Ordinal);
        var originals = new List<Entry>();
        var bodyDepth = -1;
        var originalChars = 0;
        var romajiChars = 0;
        var sawRomaji = false;
        while (reader.Read())
        {
            token.ThrowIfCancellationRequested();
            if (reader.Depth > 32) throw new LrcServiceException("Struktur TTML terlalu dalam.");
            if (reader.NodeType == System.Xml.XmlNodeType.EndElement && reader.LocalName == "body") bodyDepth = -1;
            if (reader.NodeType != System.Xml.XmlNodeType.Element) continue;
            if (reader.LocalName == "body") { bodyDepth = reader.Depth; continue; }
            if (requireRomaji && reader.LocalName == "transliteration" && !sawRomaji)
            {
                var language = reader.GetAttribute("lang", "http://www.w3.org/XML/1998/namespace") ?? "";
                if (!language.Equals("ja-Latn", StringComparison.OrdinalIgnoreCase)
                    && !language.StartsWith("ja-Latn-", StringComparison.OrdinalIgnoreCase)) continue;
                sawRomaji = true;
                using var section = reader.ReadSubtree();
                while (section.Read())
                {
                    token.ThrowIfCancellationRequested();
                    if (section.NodeType != System.Xml.XmlNodeType.Element || section.LocalName != "text") continue;
                    var entry = ReadEntry(section, token);
                    if (entry.Key.Length == 0 || entry.Text.Length == 0) continue;
                    romajiChars += entry.Text.Length;
                    if (romajiChars > LyricLimits.SourceCharacters || romanized.Count >= LyricLimits.Lines) return [];
                    if (!romanized.TryAdd(entry.Key, entry)) return [];
                }
            }
            else if (bodyDepth >= 0 && reader.LocalName == "p")
            {
                var entry = ReadEntry(reader, token);
                if (entry.Text.Length == 0) continue;
                originalChars += entry.Text.Length;
                if (originalChars > LyricLimits.SourceCharacters || originals.Count >= LyricLimits.Lines) return [];
                originals.Add(entry);
            }
        }
        if (originals.Count == 0 || (requireRomaji && (!sawRomaji || romanized.Count == 0))) return [];
        var picked = new List<Entry>(originals.Count);
        var matched = 0;
        var characters = 0;
        foreach (var original in originals)
        {
            token.ThrowIfCancellationRequested();
            Entry? romaji = null;
            var translated = requireRomaji && romanized.TryGetValue(original.Key, out romaji);
            // Lagu Jepang kadang memiliki baris Inggris. Boleh dipakai jika tidak mengandung aksara Jepang.
            if (requireRomaji && !translated && HasJapanese(original.Text)) return [];
            var text = translated ? romaji!.Text : original.Text;
            if (requireRomaji && HasJapanese(text)) return [];
            var begin = original.Begin ?? romaji?.Begin;
            var end = original.End ?? romaji?.End;
            if (begin is null || (end is double finish && finish < begin.Value)) return [];
            if (translated)
            {
                matched++;
                // Jangan menempelkan romaji yang merujuk baris/waktu berbeda.
                if (romaji!.Begin is double rb && rb < begin.Value - 2) return [];
                if (romaji.Begin is double late && end is double originalEnd && late > originalEnd + 2) return [];
                if (romaji.End is double early && early < begin.Value - 2) return [];
                if (romaji.End is double re && end is double oe && re > oe + 2) return [];
                if (romaji.Begin is double rBegin && romaji.End is double rEnd && rEnd < rBegin) return [];
            }
            if (duration > 0 && (end ?? begin.Value) > duration + 10) return [];
            characters += text.Length;
            if (characters > LyricLimits.SourceCharacters) return [];
            picked.Add(new Entry(original.Key, begin, end, text));
        }
        if (requireRomaji && matched == 0) return [];
        picked.Sort((a, b) => a.Begin!.Value.CompareTo(b.Begin!.Value));
        var output = new List<LyricLine>(Math.Min(LyricLimits.Lines, picked.Count * 2));
        for (var i = 0; i < picked.Count; i++)
        {
            var row = picked[i];
            var start = row.Begin!.Value;
            if (output.Count > 0 && Math.Abs(output[^1].Seconds - start) < 0.001)
                output[^1] = output[^1] with { Text = output[^1].Text + " / " + row.Text };
            else output.Add(new LyricLine(start, row.Text));
            var next = i + 1 < picked.Count ? picked[i + 1].Begin!.Value : double.PositiveInfinity;
            if (row.End is double end && end > start && next - end > 1.5)
                output.Add(new LyricLine(end, ""));
            if (output.Count > LyricLimits.Lines || output[^1].Text.Length > MaxLineCharacters) return [];
        }
        if (output.Any(x => x.Text.Length > MaxLineCharacters)
            || output.Sum(x => x.Text.Length + 3) > LyricLimits.SourceCharacters) return [];
        return output.ToArray();
    }

    private static Entry ReadEntry(System.Xml.XmlReader source, CancellationToken token)
    {
        var key = source.GetAttribute("for") ?? "";
        if (key.Length == 0 && source.HasAttributes)
        {
            while (source.MoveToNextAttribute())
                if (source.LocalName is "key" or "id") key = source.Value;
            source.MoveToElement();
        }
        var begin = ParseTime(source.GetAttribute("begin"));
        var end = ParseTime(source.GetAttribute("end"));
        double? firstSpan = null, lastSpan = null;
        var text = new StringBuilder();
        using var part = source.ReadSubtree();
        while (part.Read())
        {
            token.ThrowIfCancellationRequested();
            if (part.Depth > 32) throw new LrcServiceException("Struktur baris TTML terlalu dalam.");
            if (part.NodeType == System.Xml.XmlNodeType.Element && part.LocalName == "span")
            {
                if (ParseTime(part.GetAttribute("begin")) is double b) firstSpan = firstSpan is null ? b : Math.Min(firstSpan.Value, b);
                if (ParseTime(part.GetAttribute("end")) is double e) lastSpan = lastSpan is null ? e : Math.Max(lastSpan.Value, e);
            }
            if (part.NodeType is System.Xml.XmlNodeType.Text or System.Xml.XmlNodeType.CDATA
                or System.Xml.XmlNodeType.Whitespace or System.Xml.XmlNodeType.SignificantWhitespace)
            {
                if (text.Length + part.Value.Length > MaxLineCharacters)
                    throw new LrcServiceException("Baris lirik terlalu panjang.");
                text.Append(part.Value); // Spasi antarsuku kata dan tanda baca dipertahankan.
            }
            else if (part.NodeType == System.Xml.XmlNodeType.Element && part.LocalName == "br") text.Append(' ');
        }
        return new Entry(key, begin ?? firstSpan, end ?? lastSpan, text.ToString().Trim());
    }

    private static bool HasJapanese(string text)
    {
        foreach (var c in text)
            if (c is >= '\u3040' and <= '\u30ff' or >= '\u3400' and <= '\u9fff' or >= '\uff66' and <= '\uff9d') return true;
        return false;
    }

    internal static double? ParseTime(string? text)
    {
        if (string.IsNullOrWhiteSpace(text)) return null;
        text = text.Trim();
        var factor = 1.0;
        if (text.EndsWith("ms", StringComparison.Ordinal)) { factor = 0.001; text = text[..^2]; }
        else if (text.EndsWith('s')) text = text[..^1];
        var parts = text.Split(':');
        if (parts.Length is < 1 or > 3 || (factor != 1 && parts.Length != 1)) return null;
        var total = 0.0;
        for (var i = 0; i < parts.Length; i++)
        {
            if (!double.TryParse(parts[i], NumberStyles.AllowDecimalPoint, CultureInfo.InvariantCulture, out var n)
                || !double.IsFinite(n) || n < 0 || (i > 0 && n >= 60)) return null;
            total = total * 60 + n;
        }
        total *= factor;
        return total <= 86400 ? total : null;
    }
}

// Unison membaca LRC/TTML komunitas dan mengubah teks pada baris yang sama menjadi romaji.
internal static class UnisonClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static DateTimeOffset _nextRequestUtc;
    public static bool HasJapaneseText(string text)
        => Regex.IsMatch(text, @"[\u3040-\u30ff\u3400-\u9fff\uff66-\uff9d]");
    public static bool HasJapanese(LyricDocument document)
        => document.IsSynced ? document.Lines.Any(x => HasJapaneseText(x.Text)) : HasJapaneseText(document.PlainText);
    public static bool IsJapanese(LrcRecord record)
    {
        var language = record.Language.Trim().Replace('_', '-');
        if (language.Equals("ja", StringComparison.OrdinalIgnoreCase) || language.StartsWith("ja-", StringComparison.OrdinalIgnoreCase)) return true;
        if (language.Length > 0 && !language.Equals("und", StringComparison.OrdinalIgnoreCase)
            && !language.Equals("unknown", StringComparison.OrdinalIgnoreCase)) return false;
        // Kanji saja juga dapat merupakan bahasa Mandarin; Otomatis tidak menyamakannya dengan Jepang.
        return record.ParsedLines is { } lines ? lines.Any(x => LrcClient.HasKana(x.Text))
            : LrcClient.HasKana(record.SyncedLyrics ?? record.PlainLyrics ?? "");
    }
    private static string Text(JsonElement element, string name, int max = 512)
        => element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String
            && value.GetString() is string text && text.Length <= max ? text : "";

    private static LrcRecord? ParseRecord(JsonElement value, CancellationToken token, bool metadataOnly = false, bool preferRomaji = true)
    {
        if (value.ValueKind != JsonValueKind.Object) return null;
        var title = Text(value, "song");
        if (string.IsNullOrWhiteSpace(title)) return null;
        var record = new LrcRecord { TrackName = title, ArtistName = Text(value, "artist"),
            AlbumName = Text(value, "album"), Language = Text(value, "language", 32), Source = "Unison" };
        if (value.TryGetProperty("id", out var id) && id.ValueKind == JsonValueKind.Number && id.TryGetInt64(out var number)) record.Id = number;
        if (value.TryGetProperty("duration", out var duration) && duration.ValueKind == JsonValueKind.Number
            && duration.TryGetDouble(out var seconds) && double.IsFinite(seconds) && seconds is > 0 and <= 86400)
            record.Duration = seconds;
        if (metadataOnly) return record;
        var format = Text(value, "format", 16).ToLowerInvariant();
        var lyrics = Text(value, "lyrics", format == "ttml" ? 384000 : LyricLimits.SourceCharacters);
        if (lyrics.Length == 0) return null;
        if (format == "ttml")
        {
            record.ParsedLines = preferRomaji ? TtmlRomajiParser.Parse(lyrics, record.Duration, token) : [];
            record.IsRomanized = record.ParsedLines.Length > 0;
            if (!record.IsRomanized) record.ParsedLines = TtmlRomajiParser.ParseOriginal(lyrics, record.Duration, token);
            if (record.ParsedLines.Length == 0) return null;
        }
        else if (format == "lrc") record.SyncedLyrics = lyrics;
        else if (format == "plain") record.PlainLyrics = lyrics;
        else return null;
        var document = LyricDocument.From(record);
        if (document.Preview.Length == 0) return null;
        if (record.Language.StartsWith("ja-Latn", StringComparison.OrdinalIgnoreCase)
            && !HasJapanese(document)) record.IsRomanized = true;
        return record;
    }

    private static IEnumerable<JsonElement> Records(JsonDocument? document)
    {
        if (document is null || document.RootElement.ValueKind != JsonValueKind.Object
            || !document.RootElement.TryGetProperty("success", out var ok) || ok.ValueKind != JsonValueKind.True
            || !document.RootElement.TryGetProperty("data", out var data)) yield break;
        if (data.ValueKind == JsonValueKind.Object) yield return data;
        else if (data.ValueKind == JsonValueKind.Array)
            foreach (var item in data.EnumerateArray().Take(5)) yield return item;
    }

    public static async Task<List<LrcRecord>> FindAsync(string title, string artist, string album,
        double duration, bool preferRomaji, CancellationToken token)
    {
        var records = new List<LrcRecord>(5);
        if (!string.IsNullOrWhiteSpace(artist))
        {
            var path = "/lyrics?song=" + Uri.EscapeDataString(title) + "&artist=" + Uri.EscapeDataString(artist);
            if (album.Length > 0) path += "&album=" + Uri.EscapeDataString(album);
            if (duration is > 0 and <= 86400) path += "&duration=" + duration.ToString("0.###", CultureInfo.InvariantCulture);
            using var exact = await RequestAsync(path, null, token);
            foreach (var value in Records(exact))
                if (ParseRecord(value, token, preferRomaji: preferRomaji) is LrcRecord record) records.Add(record);
            if (records.Count > 0) return records;
        }
        // Search hanya berisi metadata. Ambil badan lirik melalui ID hasilnya, maksimal tiga kandidat.
        List<LyricCandidate> candidates;
        using (var search = await RequestAsync("/lyrics/search?limit=5&q=" + Uri.EscapeDataString(title + " " + artist), null, token))
        {
            var metadata = Records(search).Select(x => ParseRecord(x, token, true)).OfType<LrcRecord>();
            candidates = SongMatcher.Rank(metadata, title, artist, 0).Take(3).ToList();
        }
        foreach (var candidate in candidates)
        {
            if (candidate.Record.Id <= 0) continue;
            using var full = await RequestAsync("/lyrics/" + candidate.Record.Id.ToString(CultureInfo.InvariantCulture), null, token);
            foreach (var value in Records(full))
                if (ParseRecord(value, token, preferRomaji: preferRomaji) is LrcRecord record) records.Add(record);
        }
        return records;
    }

    public static async Task<LrcRecord> RomanizeAsync(LrcRecord source, LyricDocument document, CancellationToken token)
    {
        if (!document.IsSynced) throw new LrcServiceException("Romaji sinkron memerlukan lirik Jepang berwaktu.");
        var positions = Enumerable.Range(0, document.Lines.Length)
            .Where(i => HasJapaneseText(document.Lines[i].Text)).ToArray();
        if (positions.Length == 0) return source;
        if (positions.Length > 400 || positions.Any(i => document.Lines[i].Text.Length > 1000))
            throw new LrcServiceException("Teks terlalu besar untuk konversi otomatis. Muat file romaji .lrc yang sudah siap.");
        var output = document.Lines.ToArray();
        var chars = output.Sum(x => x.Text.Length + 3);
        var cursor = 0;
        var requests = 0;
        while (cursor < positions.Length)
        {
            token.ThrowIfCancellationRequested();
            if (++requests > 4) throw new LrcServiceException("Konversi melewati batas empat kelompok. Muat file romaji .lrc.");
            var start = cursor;
            var batch = new List<string>(100);
            var batchChars = 0;
            while (cursor < positions.Length && batch.Count < 100)
            {
                var line = document.Lines[positions[cursor]].Text;
                if (batchChars + line.Length > 12000) break;
                batch.Add(line); batchChars += line.Length; cursor++;
            }
            using var json = await RequestAsync("/translate", batch, token);
            if (json is null || json.RootElement.ValueKind != JsonValueKind.Object
                || !json.RootElement.TryGetProperty("lines", out var lines) || lines.ValueKind != JsonValueKind.Array
                || lines.GetArrayLength() != batch.Count)
                throw new LrcServiceException("Jumlah baris romaji tidak cocok. Hasil konversi tidak dipakai agar timing tidak tertukar.");
            var language = Text(json.RootElement, "detectedLang", 32);
            if (language.Length > 0 && !language.Equals("ja", StringComparison.OrdinalIgnoreCase))
                throw new LrcServiceException("Bahasa hasil konversi bukan Jepang. Periksa pilihan lagunya.");
            var item = 0;
            foreach (var line in lines.EnumerateArray())
            {
                var romaji = line.ValueKind == JsonValueKind.Object ? Text(line, "romanization", 4096).Trim() : "";
                // Jangan memakai kolom translation: itu terjemahan Inggris, bukan bacaan romaji.
                if (romaji.Length == 0 || HasJapaneseText(romaji) || !Regex.IsMatch(romaji, "[A-Za-z]")
                    || romaji.Contains('\n') || romaji.Contains('\r'))
                    throw new LrcServiceException("Romaji dari Unison belum lengkap. Hasil sebagian tidak disimpan; coba lagi nanti atau Muat file .lrc.");
                var index = positions[start + item++];
                chars += romaji.Length - output[index].Text.Length;
                if (chars > LyricLimits.SourceCharacters) throw new LrcServiceException("Hasil romaji terlalu besar.");
                // Waktu dan urutan tidak diubah. Baris Inggris, kosong, dan jeda instrumental juga tetap.
                output[index] = output[index] with { Text = romaji };
            }
        }
        return new LrcRecord { Id = source.Id, TrackName = source.TrackName, ArtistName = source.ArtistName,
            AlbumName = source.AlbumName, Duration = source.Duration, Source = source.Source + " · romaji Unison",
            IsRomanized = true, MachineRomanized = true, IsLocal = source.IsLocal, Language = "ja-Latn", ParsedLines = output };
    }

    private static async Task<JsonDocument?> RequestAsync(string path, List<string>? lines, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            var wait = _nextRequestUtc - DateTimeOffset.UtcNow;
            if (wait > TimeSpan.FromSeconds(2))
                throw new LrcServiceException($"Unison meminta jeda sekitar {Math.Ceiling(wait.TotalSeconds)} detik. Lirik lokal tetap bisa dimuat.");
            if (wait > TimeSpan.Zero) await Task.Delay(wait, token);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
            timeout.CancelAfter(TimeSpan.FromSeconds(lines is null ? 8 : 15));
            using var request = new HttpRequestMessage(lines is null ? HttpMethod.Get : HttpMethod.Post,
                "https://unison.boidu.dev" + path);
            if (lines is not null)
                request.Content = new StringContent(JsonSerializer.Serialize(new { lines, to = "en", from = "ja" }), Encoding.UTF8, "application/json");
            using var response = await LyricHttp.Client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            if (response.StatusCode == HttpStatusCode.NotFound && lines is null) return null;
            if (response.StatusCode is HttpStatusCode.TooManyRequests or HttpStatusCode.ServiceUnavailable)
            {
                var retry = response.Headers.RetryAfter;
                var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date ? date - DateTimeOffset.UtcNow : TimeSpan.FromSeconds(30));
                if (delay < TimeSpan.FromSeconds(1)) delay = TimeSpan.FromSeconds(1);
                try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                throw new LrcServiceException("Unison sedang sibuk atau meminta jeda. Hasil yang sudah tersimpan tetap dapat digunakan.");
            }
            if (!response.IsSuccessStatusCode)
                throw new LrcServiceException($"Unison belum dapat memproses lirik (HTTP {(int)response.StatusCode}). Coba lagi nanti atau Muat file .lrc.");
            return await LyricHttp.ReadJsonAsync(response, timeout.Token);
        }
        catch (OperationCanceledException) when (!token.IsCancellationRequested)
        { throw new LrcServiceException("Unison melewati batas waktu. Coba lagi nanti atau Muat file .lrc."); }
        finally
        {
            var earliest = DateTimeOffset.UtcNow.AddMilliseconds(500);
            if (_nextRequestUtc < earliest) _nextRequestUtc = earliest;
            Gate.Release();
        }
    }
}

// Romaji lokal: bacaan kanji dari penganalisis fonetik Jepang bawaan Windows,
// lalu kana diubah ke romaji Hepburn. Tanpa internet; waktu tiap baris tidak berubah.
internal static class LocalRomaji
{
    private const int AnalyzerLimit = 100; // Batas input JapanesePhoneticAnalyzer.GetWords.
    private static readonly Regex JapaneseRun = new(@"[぀-ヿ㐀-鿿豈-﫿ｦ-ﾝ々〆ヶ]+", RegexOptions.Compiled);

    public static LrcRecord Romanize(LrcRecord source, LyricDocument document, CancellationToken token)
    {
        if (!document.IsSynced) throw new LrcServiceException("Romaji sinkron memerlukan lirik Jepang berwaktu.");
        var output = document.Lines.ToArray();
        var converted = 0;
        for (var i = 0; i < output.Length; i++)
        {
            token.ThrowIfCancellationRequested();
            var text = output[i].Text;
            if (!UnisonClient.HasJapaneseText(text)) continue;
            var romaji = Line(text);
            if (romaji.Length == 0 || UnisonClient.HasJapaneseText(romaji))
                throw new LrcServiceException("Sebagian baris belum bisa dibaca oleh penganalisis Jepang Windows.");
            output[i] = output[i] with { Text = romaji }; // Hanya teks yang diganti; detik baris tetap.
            converted++;
        }
        if (converted == 0) return source;
        return new LrcRecord { Id = source.Id, TrackName = source.TrackName, ArtistName = source.ArtistName,
            AlbumName = source.AlbumName, Duration = source.Duration, Source = source.Source + " · romaji lokal",
            IsRomanized = true, MachineRomanized = true, IsLocal = source.IsLocal, Language = "ja-Latn", ParsedLines = output };
    }

    // Satu baris lirik: bagian Jepang dibaca per kata, bagian lain (Inggris, angka, tanda baca) dipertahankan.
    internal static string Line(string text)
    {
        var words = new List<string>();
        var last = 0;
        foreach (Match run in JapaneseRun.Matches(text))
        {
            AddOther(words, text[last..run.Index]);
            foreach (var chunk in Chunks(run.Value))
            {
                var phonemes = global::Windows.Globalization.JapanesePhoneticAnalyzer.GetWords(chunk, false);
                foreach (var phoneme in phonemes)
                {
                    var display = phoneme.DisplayText ?? "";
                    var yomi = string.IsNullOrEmpty(phoneme.YomiText) ? display : phoneme.YomiText;
                    // Partikel ditulis menurut bunyinya: は = wa, へ = e, を = wo.
                    var romaji = !phoneme.IsPhraseStart && display == "は" ? "wa"
                        : !phoneme.IsPhraseStart && display == "へ" ? "e"
                        : display == "を" ? "wo"
                        : Kana(yomi);
                    if (romaji.Length == 0) continue;
                    // Konsonan ganda (っ) di akhir kata digabung dengan kata berikutnya.
                    if (words.Count > 0 && words[^1].EndsWith('\u0001'))
                        words[^1] = words[^1][..^1] + (romaji.StartsWith("ch") ? "t"
                            : "aiueo".Contains(romaji[0]) ? "" : romaji[0].ToString()) + romaji;
                    else words.Add(romaji);
                }
            }
            last = run.Index + run.Length;
        }
        AddOther(words, text[last..]);
        var line = string.Join(" ", words).Replace("\u0001", "");
        line = Regex.Replace(line, @"\s+([,.!?~)\]」』])", "$1");
        line = Regex.Replace(line, @"([(\[「『])\s+", "$1");
        return Regex.Replace(line, @"\s{2,}", " ").Trim();
    }

    private static void AddOther(List<string> words, string text)
    {
        text = text.Replace('　', ' ').Replace("、", ", ").Replace("。", ". ").Replace("！", "! ").Replace("？", "? ")
            .Replace("～", "~").Replace("〜", "~").Replace("・", " ").Replace("（", "(").Replace("）", ")")
            .Replace("「", "\"").Replace("」", "\"").Replace("『", "\"").Replace("』", "\"").Replace("…", "...");
        foreach (var part in text.Split(' ', StringSplitOptions.RemoveEmptyEntries)) words.Add(part);
    }

    private static IEnumerable<string> Chunks(string run)
    {
        for (var i = 0; i < run.Length; i += AnalyzerLimit)
            yield return run.Substring(i, Math.Min(AnalyzerLimit, run.Length - i));
    }

    private static readonly Dictionary<string, string> Table = BuildTable();

    private static Dictionary<string, string> BuildTable()
    {
        var t = new Dictionary<string, string>(StringComparer.Ordinal);
        void Row(string kana, params string[] romaji) { for (var i = 0; i < kana.Length; i++) t[kana[i].ToString()] = romaji[i]; }
        Row("あいうえお", "a", "i", "u", "e", "o");
        Row("かきくけこ", "ka", "ki", "ku", "ke", "ko"); Row("がぎぐげご", "ga", "gi", "gu", "ge", "go");
        Row("さしすせそ", "sa", "shi", "su", "se", "so"); Row("ざじずぜぞ", "za", "ji", "zu", "ze", "zo");
        Row("たちつてと", "ta", "chi", "tsu", "te", "to"); Row("だぢづでど", "da", "ji", "zu", "de", "do");
        Row("なにぬねの", "na", "ni", "nu", "ne", "no");
        Row("はひふへほ", "ha", "hi", "fu", "he", "ho"); Row("ばびぶべぼ", "ba", "bi", "bu", "be", "bo");
        Row("ぱぴぷぺぽ", "pa", "pi", "pu", "pe", "po");
        Row("まみむめも", "ma", "mi", "mu", "me", "mo");
        Row("やゆよ", "ya", "yu", "yo");
        Row("らりるれろ", "ra", "ri", "ru", "re", "ro");
        Row("わゐゑを", "wa", "wi", "we", "o");
        Row("ぁぃぅぇぉゃゅょゎ", "a", "i", "u", "e", "o", "ya", "yu", "yo", "wa");
        t["ゔ"] = "vu"; t["ゕ"] = "ka"; t["ゖ"] = "ke";
        // Gabungan yōon: き+ゃ = kya, し+ゃ = sha, dst.
        var yoon = new (string Kana, string Head)[] { ("き", "ky"), ("ぎ", "gy"), ("し", "sh"), ("じ", "j"), ("ち", "ch"),
            ("ぢ", "j"), ("に", "ny"), ("ひ", "hy"), ("び", "by"), ("ぴ", "py"), ("み", "my"), ("り", "ry") };
        foreach (var (kana, head) in yoon)
        { t[kana + "ゃ"] = head + "a"; t[kana + "ゅ"] = head + "u"; t[kana + "ょ"] = head + "o"; }
        t["しぇ"] = "she"; t["じぇ"] = "je"; t["ちぇ"] = "che"; t["てぃ"] = "ti"; t["でぃ"] = "di"; t["とぅ"] = "tu"; t["どぅ"] = "du";
        t["ふぁ"] = "fa"; t["ふぃ"] = "fi"; t["ふぇ"] = "fe"; t["ふぉ"] = "fo"; t["ふゅ"] = "fyu";
        t["うぃ"] = "wi"; t["うぇ"] = "we"; t["うぉ"] = "wo"; t["つぁ"] = "tsa"; t["つぃ"] = "tsi"; t["つぇ"] = "tse"; t["つぉ"] = "tso";
        t["ゔぁ"] = "va"; t["ゔぃ"] = "vi"; t["ゔぇ"] = "ve"; t["ゔぉ"] = "vo"; t["いぇ"] = "ye"; t["くぁ"] = "kwa";
        return t;
    }

    // Kana (hiragana/katakana) ke romaji Hepburn. っ menggandakan konsonan, ー memanjangkan vokal,
    // dan ん sebelum vokal/y diberi apostrof agar tidak terbaca salah (kan'i, bukan kani).
    internal static string Kana(string input)
    {
        var text = new StringBuilder(input.Length);
        foreach (var c in input) text.Append(c is >= 'ァ' and <= 'ヶ' ? (char)(c - 0x60) : c); // Katakana -> hiragana.
        var s = text.ToString();
        var result = new StringBuilder(s.Length * 2);
        var doubleNext = false;
        for (var i = 0; i < s.Length; i++)
        {
            var c = s[i];
            if (c == 'っ') { doubleNext = true; continue; }
            if (c == 'ん')
            {
                result.Append('n');
                if (i + 1 < s.Length && Table.TryGetValue(s[i + 1].ToString(), out var after) && after.Length > 0 && "aiueoy".Contains(after[0]))
                    result.Append('\'');
                continue;
            }
            if (c is 'ー' or 'ｰ')
            {
                var vowel = result.Length > 0 ? result[^1] : '\0';
                if ("aiueo".Contains(vowel)) result.Append(vowel);
                continue;
            }
            string? romaji = null;
            if (i + 1 < s.Length && Table.TryGetValue(s.Substring(i, 2), out var pair)) { romaji = pair; i++; }
            else if (Table.TryGetValue(c.ToString(), out var single)) romaji = single;
            if (romaji is null)
            {
                // Tanda kana tanpa bunyi (mis. ・ atau ゛ terpisah) dibuang; karakter lain dipertahankan.
                if (!char.IsWhiteSpace(c) && c is not (>= '\u3040' and <= '\u30ff')) result.Append(c);
                doubleNext = false; continue;
            }
            if (doubleNext)
            {
                if (romaji.StartsWith("ch")) result.Append('t');
                else if (!"aiueo".Contains(romaji[0])) result.Append(romaji[0]);
                doubleNext = false;
            }
            result.Append(romaji);
        }
        // っ di akhir kata (mis. "あっ" atau kata yang terpotong analyzer) disambung ke kata berikutnya.
        if (doubleNext) result.Append('\u0001');
        return result.ToString();
    }
}

// Satu dokumen aktif di RAM. Lagu-lagu lain tetap berupa file kecil di disk, tanpa indeks katalog di RAM.
internal sealed class CachedLyrics
{
    public CachedLyrics() { }
    public int Schema { get; set; } = 1;
    public string Identity { get; set; } = "";
    public LyricsMode Mode { get; set; }
    public double PlaybackDuration { get; set; }
    public double OffsetSeconds { get; set; }
    public string Title { get; set; } = "";
    public string Artist { get; set; } = "";
    public string Album { get; set; } = "";
    public string Source { get; set; } = "";
    public string Language { get; set; } = "";
    public double Duration { get; set; }
    public bool Instrumental { get; set; }
    public bool IsRomanized { get; set; }
    public bool MachineRomanized { get; set; }
    public LyricLine[] Lines { get; set; } = [];
    public string PlainText { get; set; } = "";

    public static CachedLyrics Create(string identity, LyricsMode mode, double playbackDuration, double offset,
        LrcRecord record, LyricDocument document) => new()
    {
        Identity = identity, Mode = mode, PlaybackDuration = playbackDuration, OffsetSeconds = offset,
        Title = record.TrackName, Artist = record.ArtistName, Album = record.AlbumName, Source = record.Source,
        Language = record.Language, Duration = record.Duration, Instrumental = record.Instrumental,
        IsRomanized = LrcClient.IsMarkedRomaji(record), MachineRomanized = record.MachineRomanized,
        Lines = document.Lines, PlainText = document.IsSynced ? "" : document.PlainText
    };

    public LrcRecord ToRecord() => new() { TrackName = Title, ArtistName = Artist, AlbumName = Album,
        Duration = Duration, Instrumental = Instrumental, Source = Source, Language = Language,
        IsRomanized = IsRomanized, MachineRomanized = MachineRomanized, IsLocal = true,
        ParsedLines = Lines, PlainLyrics = PlainText };

    public bool IsValid()
    {
        if (Schema != 1 || Identity is null || Identity.Length is < 1 or > 4096
            || (int)Mode is < 0 or > 2
            || !double.IsFinite(OffsetSeconds) || Math.Abs(OffsetSeconds) > 120
            || !double.IsFinite(PlaybackDuration) || PlaybackDuration is < 0 or > 86400
            || !double.IsFinite(Duration) || Duration is < 0 or > 86400
            || new[] { Title, Artist, Album, Source, Language }.Any(x => x is null || x.Length > 512)
            || Lines is null || Lines.Length > LyricLimits.Lines || PlainText is null
            || PlainText.Length > LyricLimits.SourceCharacters || (Lines.Length > 0 && PlainText.Length > 0)) return false;
        var previous = -1.0;
        var chars = 0;
        foreach (var line in Lines)
        {
            if (line is null || !double.IsFinite(line.Seconds) || line.Seconds is < 0 or > 86400
                || line.Seconds <= previous || line.Text is null || line.Text.Length > 4096
                || line.Text.Contains('\n') || line.Text.Contains('\r')) return false;
            previous = line.Seconds; chars += line.Text.Length + 3;
            if (chars > LyricLimits.SourceCharacters) return false;
        }
        return Instrumental || (Lines.Length > 0 ? Lines.Any(x => !string.IsNullOrWhiteSpace(x.Text)) : !string.IsNullOrWhiteSpace(PlainText));
    }
}

// Ringkasan cache tidak memuat Lines/PlainText. Cache lama tetap dibaca dengan skema yang sama.
internal sealed class LyricCacheHeader
{
    public LyricCacheHeader() { }
    public int Schema { get; set; } = 1;
    public string Identity { get; set; } = "";
    public LyricsMode Mode { get; set; }
    public double PlaybackDuration { get; set; }
    public double Duration { get; set; }
    public string Title { get; set; } = "";
    public string Artist { get; set; } = "";
    public string Album { get; set; } = "";
    public bool IsRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public double RecordingDuration => PlaybackDuration > 0 ? PlaybackDuration : Duration;

    public static LyricCacheHeader From(CachedLyrics entry) => new()
    {
        Schema = entry.Schema, Identity = entry.Identity, Mode = entry.Mode,
        PlaybackDuration = entry.PlaybackDuration, Duration = entry.Duration,
        Title = entry.Title, Artist = entry.Artist, Album = entry.Album, IsRomanized = entry.IsRomanized
    };
    public bool IsValid() => Schema == 1 && Identity is { Length: > 0 and <= 4096 }
        && (int)Mode is >= 0 and <= 2 && Identity.Split('\u001f').Length == 3
        && new[] { Title, Artist, Album }.All(x => x is not null && x.Length <= 512)
        && double.IsFinite(PlaybackDuration) && PlaybackDuration is >= 0 and <= 86400
        && double.IsFinite(Duration) && Duration is >= 0 and <= 86400;
}

internal sealed record CacheTrackNames(string Title, string Artist, string ArtistKey, string[] Titles, string Version);
internal sealed record CacheSongMatch(double Score, bool Exact);

internal static class CacheSongMatcher
{
    private const string VersionWords = @"\b(?:live|remix|cover|acoustic|instrumental|karaoke|remaster(?:ed)?|version|ver|edit|radio|extended|demo|session|piano|sped\s*up|speed\s*up|slowed|nightcore|8d|mono|stereo|re[- ]?recorded|part|pt|vol|volume|movement|chapter|feat|ft)\b|ライブ|カバー|リミックス|アレンジ";
    private const RegexOptions Flags = RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;

    private static string CleanArtist(string artist)
    {
        artist = SongMatcher.CleanArtist(artist);
        return Regex.Replace(artist, @"(?:\s*(?:[-–—|]\s*)?VEVO|\s+(?:[-–—|]\s*)?(?:Official(?:\s+(?:YouTube\s+)?Channel)?|Topic|Topik))\s*$", "", Flags).Trim();
    }
    private static string Key(string text)
        => new(text.Normalize(NormalizationForm.FormKC).Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());

    public static CacheTrackNames Names(string title, string artist)
    {
        artist = CleanArtist(artist);
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        // Hanya label kemasan video yang dibuang. Label versi rekaman tetap diperiksa di bawah.
        title = Regex.Replace(title, @"[\(\[【](?:\s*(?:official\s+)?(?:lyrics?|lyric\s+video|audio|music\s+video|video|mv|pv|visuali[sz]er|hd|4k|\d{3,4}p)\s*)[\)\]】]", " ", Flags);
        title = Regex.Replace(title, @"\s+[-–—|]\s*(?:lyrics?|audio|visuali[sz]er)\s*$", "", Flags).Trim();
        title = Regex.Replace(title, @"[\(\[【]\s*[\)\]】]", "").Trim();
        var aliases = new List<string> { title };
        foreach (Match part in Regex.Matches(title, @"[\(（]([^\)）]+)[\)）]").Cast<Match>().Take(4))
        {
            var inside = part.Groups[1].Value.Trim();
            var outside = title.Remove(part.Index, part.Length).Trim();
            // Alias beda aksara harus tertulis di metadata; jangan membuang subjudul/nomor bagian sembarang.
            if (!Regex.IsMatch(title, VersionWords, Flags) && inside.Length > 0 && outside.Length > 0
                && UnisonClient.HasJapaneseText(inside) != UnisonClient.HasJapaneseText(outside))
            { aliases.Add(inside); aliases.Add(outside); }
        }
        var keys = aliases.Select(Key).Where(x => x.Length > 0).Distinct().Take(6).ToArray();
        return new(title, artist, Key(artist), keys, VersionKey(title));
    }
    public static CacheTrackNames FromIdentity(string identity)
    {
        var parts = identity.Split('\u001f');
        return Names(parts.ElementAtOrDefault(0) ?? "", parts.ElementAtOrDefault(1) ?? "");
    }

    private static string VersionKey(string title)
    {
        if (!Regex.IsMatch(title, VersionWords, Flags)) return "";
        var labels = new List<string>();
        foreach (Match part in Regex.Matches(title, @"[\(\[【（]([^\)\]】）]+)[\)\]】）]"))
            if (Regex.IsMatch(part.Groups[1].Value, VersionWords, Flags)) labels.Add(Key(part.Groups[1].Value));
        var tail = Regex.Match(title, @"\s[-–—|]\s(?<tail>.+)$");
        if (tail.Success && Regex.IsMatch(tail.Groups["tail"].Value, VersionWords, Flags)) labels.Add(Key(tail.Groups["tail"].Value));
        labels.AddRange(Regex.Matches(title, VersionWords, Flags).Cast<Match>().Select(x => Key(x.Value)));
        labels.AddRange(Regex.Matches(title, @"\b(?:19|20)\d{2}\b").Cast<Match>().Select(x => x.Value));
        return string.Join("|", labels.Distinct().OrderBy(x => x, StringComparer.Ordinal));
    }

    private static double TitleSimilarity(CacheTrackNames first, CacheTrackNames second)
    {
        if (first.Titles.Intersect(second.Titles, StringComparer.Ordinal).Any()) return 1;
        // Fuzzy sangat ketat untuk variasi ejaan Latin. Aksara Jepang memakai alias yang benar-benar tersimpan.
        if (UnisonClient.HasJapaneseText(first.Title) || UnisonClient.HasJapaneseText(second.Title)) return 0;
        var best = 0.0;
        foreach (var a in first.Titles)
            foreach (var b in second.Titles)
                if (a.Length >= 8 && b.Length >= 8
                    && Regex.Matches(a, @"\d+").Cast<Match>().Select(x => x.Value)
                        .SequenceEqual(Regex.Matches(b, @"\d+").Cast<Match>().Select(x => x.Value)))
                    best = Math.Max(best, SongMatcher.Similarity(a, b));
        return best;
    }

    public static CacheSongMatch? Match(LyricCacheHeader cached, CacheTrackNames requested, LyricsMode mode, double duration)
    {
        if (!cached.IsValid() || (mode != LyricsMode.Auto && cached.Mode != mode) || requested.ArtistKey.Length == 0
            || !double.IsFinite(duration) || duration <= 0 || cached.RecordingDuration <= 0
            || Math.Abs(duration - cached.RecordingDuration) > 3) return null;
        var playback = FromIdentity(cached.Identity);
        var canonical = Names(cached.Title, cached.Artist);
        var storedVersion = playback.Version.Length > 0 ? playback.Version : canonical.Version;
        if (!string.Equals(requested.Version, storedVersion, StringComparison.Ordinal)) return null;
        var score = 0.0;
        foreach (var candidate in new[] { playback, canonical })
            if (requested.ArtistKey == candidate.ArtistKey)
                score = Math.Max(score, TitleSimilarity(requested, candidate));
        return score >= 0.96 ? new(score, score == 1) : null;
    }

    // Dipakai hanya untuk urutan pilihan manual; tidak pernah menjadi keputusan otomatis.
    public static double SuggestionScore(LyricCacheHeader cached, CacheTrackNames requested, LyricsMode mode, double duration)
    {
        var score = 0.0;
        foreach (var candidate in new[] { FromIdentity(cached.Identity), Names(cached.Title, cached.Artist) })
        {
            var artist = requested.ArtistKey.Length > 0 && requested.ArtistKey == candidate.ArtistKey ? 1
                : SongMatcher.Similarity(requested.Artist, candidate.Artist);
            score = Math.Max(score, 0.7 * TitleSimilarity(requested, candidate) + 0.3 * artist);
        }
        if (duration > 0 && cached.RecordingDuration > 0 && Math.Abs(duration - cached.RecordingDuration) <= 3) score += 0.15;
        if (mode == cached.Mode) score += 0.05;
        return score;
    }
}

internal static class LyricCache
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static readonly JsonSerializerOptions Options = new() { MaxDepth = 12 };
    private const int HeaderBytes = 64 * 1024;
    private static string DirectoryPath => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "NotchMusic", "Lyrics");
    private static string HeaderDirectory => Path.Combine(DirectoryPath, "lookup-v1");
    public static string Identity(string title, string artist, string album)
    {
        // Pertahankan format v4.5 agar semua file lama tetap dapat ditemukan persis.
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        static string Normalize(string value) => Regex.Replace(value[..Math.Min(value.Length, 512)]
            .Normalize(NormalizationForm.FormKC).Trim(), @"\s+", " ").ToUpperInvariant();
        return Normalize(title) + "\u001f" + Normalize(artist) + "\u001f" + Normalize(album);
    }
    private static string FilePath(string identity, LyricsMode mode)
    {
        var key = Encoding.UTF8.GetBytes(identity + "\n" + (int)mode);
        return Path.Combine(DirectoryPath, Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(key)) + ".json");
    }
    private static bool IsCacheName(string name) => Regex.IsMatch(name, @"\A[0-9A-Fa-f]{64}\.json\z", RegexOptions.CultureInvariant);
    private static bool ReadFailure(Exception ex) => ex is IOException or UnauthorizedAccessException or JsonException or ArgumentException;

    private static async Task<CachedLyrics?> ReadFileAsync(string path, CancellationToken token)
    {
        try
        {
            if (!File.Exists(path)) return null;
            await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
            if (stream.Length > LyricFiles.MaxBytes) return null;
            var entry = await JsonSerializer.DeserializeAsync<CachedLyrics>(stream, Options, token);
            return entry is not null && entry.IsValid()
                && Path.GetFileName(FilePath(entry.Identity, entry.Mode)) == Path.GetFileName(path) ? entry : null;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
    }

    private static async Task WriteHeaderQuietlyAsync(string path, LyricCacheHeader header)
    {
        try
        {
            Directory.CreateDirectory(HeaderDirectory);
            var json = JsonSerializer.Serialize(header, Options);
            if (Encoding.UTF8.GetByteCount(json) <= HeaderBytes)
                await LyricFiles.WriteAtomicAsync(Path.Combine(HeaderDirectory, Path.GetFileName(path)), json);
        }
        catch (Exception ex) when (ReadFailure(ex)) { Debug.WriteLine("Ringkasan cache belum dapat disimpan."); }
    }

    private static async Task<LyricCacheHeader?> ReadHeaderAsync(string path, CancellationToken token)
    {
        try
        {
            var data = new FileInfo(path);
            if (!data.Exists || data.Length > LyricFiles.MaxBytes) return null;
            var summary = new FileInfo(Path.Combine(HeaderDirectory, data.Name));
            LyricCacheHeader? header = null;
            if (summary.Exists && summary.Length <= HeaderBytes && summary.LastWriteTimeUtc >= data.LastWriteTimeUtc)
            {
                try
                {
                    await using var stream = new FileStream(summary.FullName, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
                    header = await JsonSerializer.DeserializeAsync<LyricCacheHeader>(stream, Options, token);
                }
                catch (Exception ex) when (ReadFailure(ex)) { }
                if (header?.IsValid() == true && Path.GetFileName(FilePath(header.Identity, header.Mode)) == data.Name) return header;
            }
            // Migrasi ringan: deserialisasi metadata saja, tanpa membuat array objek Lines dari cache lama.
            await using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true))
            {
                if (stream.Length > LyricFiles.MaxBytes) return null;
                header = await JsonSerializer.DeserializeAsync<LyricCacheHeader>(stream, Options, token);
            }
            if (header?.IsValid() != true || Path.GetFileName(FilePath(header.Identity, header.Mode)) != data.Name) return null;
            token.ThrowIfCancellationRequested();
            await WriteHeaderQuietlyAsync(path, header);
            return header;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
    }

    private sealed record Candidate(string Path, LyricCacheHeader Header, CacheSongMatch Match, DateTime Modified);

    public static async Task<CachedLyrics?> ReadAsync(string identity, LyricsMode mode, double duration, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            // Otomatis tetap dapat membaca simpanan v4.5 dari mode Romaji/Asli; tidak perlu mengunduh ulang.
            var modes = mode == LyricsMode.Auto ? new[] { LyricsMode.Auto, LyricsMode.Romaji, LyricsMode.Original } : new[] { mode };
            var exactPaths = modes.Select(m => FilePath(identity, m)).ToArray();
            for (var i = 0; i < modes.Length; i++)
            {
                token.ThrowIfCancellationRequested();
                var exact = await ReadFileAsync(exactPaths[i], token);
                if (exact is not null && exact.Identity == identity && exact.Mode == modes[i]
                    && (duration <= 0 || exact.PlaybackDuration <= 0 || Math.Abs(duration - exact.PlaybackDuration) <= 3)) return exact;
            }
            if (!Directory.Exists(DirectoryPath) || duration <= 0) return null;
            var requested = CacheSongMatcher.FromIdentity(identity);
            Candidate? best = null;
            var ambiguous = false;
            foreach (var path in Directory.EnumerateFiles(DirectoryPath, "*.json", SearchOption.TopDirectoryOnly))
            {
                token.ThrowIfCancellationRequested();
                if (!IsCacheName(Path.GetFileName(path)) || exactPaths.Contains(path)) continue;
                var header = await ReadHeaderAsync(path, token);
                if (header is null || CacheSongMatcher.Match(header, requested, mode, duration) is not CacheSongMatch match) continue;
                var candidate = new Candidate(path, header, match, File.GetLastWriteTimeUtc(path));
                if (best is null) { best = candidate; continue; }
                var equivalent = match.Exact && best.Match.Exact
                    && Math.Abs(header.RecordingDuration - best.Header.RecordingDuration) <= 1;
                if (equivalent)
                {
                    // Beberapa sumber dapat sudah menyimpan rekaman yang sama; pilih koreksi/hasil yang terakhir disimpan.
                    if ((candidate.Header.Mode == mode && best.Header.Mode != mode)
                        || ((candidate.Header.Mode == mode) == (best.Header.Mode == mode) && candidate.Modified > best.Modified)) best = candidate;
                }
                else if (match.Score - best.Match.Score >= 0.03) { best = candidate; ambiguous = false; }
                else if (best.Match.Score - match.Score < 0.03)
                {
                    ambiguous = true;
                    if (match.Score > best.Match.Score) best = candidate;
                }
            }
            if (best is null || ambiguous) return null;
            var saved = await ReadFileAsync(best.Path, token);
            // Periksa lagi badan file yang dipilih: ringkasan lama/rusak tidak boleh menghubungkan lagu keliru.
            return saved is not null && CacheSongMatcher.Match(LyricCacheHeader.From(saved), requested, mode, duration) is not null
                ? saved : null;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
        finally { Gate.Release(); }
    }

    public static async Task<List<LyricCandidate>> ListChoicesAsync(string title, string artist, LyricsMode mode,
        double duration, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            if (!Directory.Exists(DirectoryPath)) return [];
            var requested = CacheSongMatcher.Names(title, artist);
            var top = new List<(string Name, LyricCacheHeader Header, double Score, DateTime Modified)>(9);
            foreach (var path in Directory.EnumerateFiles(DirectoryPath, "*.json", SearchOption.TopDirectoryOnly))
            {
                token.ThrowIfCancellationRequested();
                var name = Path.GetFileName(path);
                if (!IsCacheName(name)) continue;
                var header = await ReadHeaderAsync(path, token);
                if (header is null) continue;
                var score = CacheSongMatcher.SuggestionScore(header, requested, mode, duration);
                top.Add((name, header, score, File.GetLastWriteTimeUtc(path)));
                top.Sort((a, b) => a.Score != b.Score ? b.Score.CompareTo(a.Score) : b.Modified.CompareTo(a.Modified));
                if (top.Count > LyricLimits.Candidates) top.RemoveAt(top.Count - 1);
            }
            // Hanya ringkasan untuk menu; teks lengkap baru dibaca setelah pengguna memilih.
            return top.Select(x => new LyricCandidate(new LrcRecord { TrackName = x.Header.Title, ArtistName = x.Header.Artist,
                AlbumName = x.Header.Album, Duration = x.Header.RecordingDuration, Source = "Lirik tersimpan", IsLocal = true,
                IsRomanized = x.Header.IsRomanized, CacheFileName = x.Name }, x.Score, false)).ToList();
        }
        catch (Exception ex) when (ReadFailure(ex)) { return []; }
        finally { Gate.Release(); }
    }

    public static async Task<CachedLyrics?> ReadChosenAsync(string fileName, CancellationToken token)
    {
        if (!IsCacheName(fileName)) return null;
        await Gate.WaitAsync(token);
        try { return await ReadFileAsync(Path.Combine(DirectoryPath, fileName), token); }
        finally { Gate.Release(); }
    }

    public static async Task<bool> WriteAsync(CachedLyrics entry)
    {
        if (!entry.IsValid()) return false;
        await Gate.WaitAsync();
        string? temporary = null;
        try
        {
            Directory.CreateDirectory(DirectoryPath);
            var path = FilePath(entry.Identity, entry.Mode);
            temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            await using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, true))
            {
                await JsonSerializer.SerializeAsync(stream, entry, Options);
                await stream.FlushAsync();
                if (stream.Length > LyricFiles.MaxBytes) return false;
            }
            File.Move(temporary, path, true);
            await WriteHeaderQuietlyAsync(path, LyricCacheHeader.From(entry));
            return true;
        }
        catch (Exception ex) when (ReadFailure(ex)) { Debug.WriteLine("Cache lirik belum dapat disimpan."); return false; }
        finally { LyricFiles.DeleteTemporary(temporary); Gate.Release(); }
    }
}

internal static class LyricFiles
{
    public const int MaxBytes = 512 * 1024;
    private static string Header(string value) => new string(value.Take(512)
        .Select(c => char.IsControl(c) || c == ']' ? ' ' : c).ToArray()).Trim();
    public static string FileName(LrcRecord record)
    {
        var name = "NotchMusic - " + record.ArtistName + " - " + record.TrackName
            + (LrcClient.IsMarkedRomaji(record) ? " (romaji)" : "");
        var invalid = Path.GetInvalidFileNameChars();
        return new string(name.Take(140).Select(c => invalid.Contains(c) || char.IsControl(c) ? '_' : c).ToArray()).TrimEnd(' ', '.');
    }
    public static string Export(LrcRecord record, LyricDocument document, double offset)
    {
        if (!document.IsSynced) return document.PlainText;
        var text = new StringBuilder();
        text.AppendLine("[ti:" + Header(record.TrackName) + "]");
        text.AppendLine("[ar:" + Header(record.ArtistName) + "]");
        text.AppendLine("[al:" + Header(record.AlbumName) + "]");
        text.AppendLine("[by:" + Header(record.Source) + "]");
        text.AppendLine($"[re:NotchMusic v{AppInfo.Version}]");
        text.AppendLine("[notch-romaji:" + (LrcClient.IsMarkedRomaji(record) ? "1" : "0") + "]");
        text.AppendLine("[notch-auto-romaji:" + (record.MachineRomanized ? "1" : "0") + "]");
        // LRC offset positif memajukan lirik; koreksi positif di UI menundanya.
        text.AppendLine("[offset:" + Math.Round(-offset * 1000).ToString("0", CultureInfo.InvariantCulture) + "]");
        foreach (var line in document.Lines)
        {
            var ms = (long)Math.Round(line.Seconds * 1000, MidpointRounding.AwayFromZero);
            text.Append('[').Append((ms / 60000).ToString("00", CultureInfo.InvariantCulture)).Append(':')
                .Append((ms / 1000 % 60).ToString("00", CultureInfo.InvariantCulture)).Append('.')
                .Append((ms % 1000).ToString("000", CultureInfo.InvariantCulture)).Append(']').AppendLine(line.Text);
        }
        return text.ToString();
    }
    public static async Task<LrcRecord> ReadAsync(string path, string title, string artist, LyricsMode mode, CancellationToken token)
    {
        var extension = Path.GetExtension(path).ToLowerInvariant();
        if (extension is not ".lrc" and not ".txt") throw new LrcServiceException("Pilih file .lrc atau .txt.");
        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
        if (stream.Length > MaxBytes) throw new LrcServiceException("File lirik maksimal 512 KiB.");
        using var reader = new StreamReader(stream, new UTF8Encoding(false, true), true, 4096, true);
        var text = await reader.ReadToEndAsync(token);
        if (text.Length > LyricLimits.TextFileCharacters) throw new LrcServiceException("Isi file lirik melewati batas ukuran.");
        string Tag(string name)
        {
            var match = Regex.Match(text, @"(?m)^\[" + Regex.Escape(name) + @":([^\]\r\n]{0,512})\]\r?$");
            return match.Success ? match.Groups[1].Value.Trim() : "";
        }
        var record = new LrcRecord { TrackName = title, ArtistName = artist, Source = "File lokal", IsLocal = true };
        if (extension == ".lrc")
        {
            if (Tag("ti").Length > 0) record.TrackName = Tag("ti");
            if (Tag("ar").Length > 0) record.ArtistName = Tag("ar");
            record.AlbumName = Tag("al");
            if (double.TryParse(Tag("offset"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var offsetMs)
                && double.IsFinite(offsetMs) && Math.Abs(offsetMs) <= 120000)
            {
                record.ImportedOffsetSeconds = -offsetMs / 1000;
                // Simpan koreksi terpisah agar waktu negatif tidak terlipat menjadi baris 00:00 yang sama.
                text = Regex.Replace(text, @"(?im)^\[offset:[^\]\r\n]*\]\r?$", "[offset:0]");
            }
            record.SyncedLyrics = text;
            record.IsRomanized = Tag("notch-romaji") == "1";
            record.MachineRomanized = record.IsRomanized && Tag("notch-auto-romaji") == "1";
        }
        else record.PlainLyrics = text;
        var document = LyricDocument.From(record);
        if (document.Preview.Length == 0 || (extension == ".lrc" && !document.IsSynced))
            throw new LrcServiceException("File kosong atau timestamp LRC tidak valid. Contoh format: [00:12.500]Teks baris.");
        // Teks Latin bisa berupa bahasa Indonesia/Inggris; pilihan mode bukan bukti bahwa teks itu romaji.
        return record;
    }
    public static async Task WriteAtomicAsync(string path, string content)
    {
        if (Encoding.UTF8.GetByteCount(content) > MaxBytes) throw new LrcServiceException("File hasil terlalu besar untuk disimpan.");
        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            await File.WriteAllTextAsync(temporary, content, new UTF8Encoding(false));
            File.Move(temporary, path, true);
        }
        finally { DeleteTemporary(temporary); }
    }
    public static void DeleteTemporary(string? path)
    {
        if (path is null) return;
        try { File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
}
