using System.Windows;

namespace NotchMusic;

public partial class App : Application
{
    private Mutex? _instance;

    protected override void OnStartup(StartupEventArgs e)
    {
        _instance = new Mutex(true, "Local\\NotchMusic.SingleInstance", out var created);
        if (!created)
        {
            DesktopNative.RestoreExistingNotch();
            _instance.Dispose(); _instance = null;
            Shutdown();
            return;
        }
        // Render dibiarkan otomatis: GPU bila ada (animasi halus), software bila tidak ada GPU.
        base.OnStartup(e);
    }

    protected override void OnExit(ExitEventArgs e)
    {
        if (_instance is not null)
        {
            _instance.ReleaseMutex(); _instance.Dispose(); _instance = null;
        }
        base.OnExit(e);
    }
}
