using System;
using System.Globalization;
using System.IO;
using System.Text;
using System.Windows.Forms;
using Aveva.ApplicationFramework;
using Aveva.Core.Utilities.CommandLine;

namespace GenModel.E3D.IncrementalTest
{
    public sealed class IncrementalTestAddin : IAddin
    {
        private Timer timer;

        public string Name { get { return "GenModel Incremental Test"; } }
        public string Description { get { return "Runs one opt-in E3D macro after startup."; } }

        public void Start(ServiceManager serviceManager)
        {
            if (string.IsNullOrEmpty(Environment.GetEnvironmentVariable("GENMODEL_E3D_MACRO"))) return;
            Application.Idle += OnIdle;
        }

        public void Stop()
        {
            if (timer == null) return;
            timer.Stop();
            timer.Dispose();
            timer = null;
        }

        private void OnIdle(object sender, EventArgs e)
        {
            Application.Idle -= OnIdle;
            timer = new Timer();
            timer.Interval = DelayMs();
            timer.Tick += OnTick;
            timer.Start();
        }

        private void OnTick(object sender, EventArgs e)
        {
            timer.Stop();
            string macro = Environment.GetEnvironmentVariable("GENMODEL_E3D_MACRO");
            try
            {
                string command = "$M \"" + macro.Replace('\\', '/') + "\"";
                Command c = Command.CreateCommand(command);
                bool ok = c.RunInPdms();
                Log(command + " -> " + ok.ToString());
            }
            catch (Exception ex)
            {
                Log("ERROR " + ex.GetType().Name + ": " + ex.Message);
            }
            if (Environment.GetEnvironmentVariable("GENMODEL_E3D_QUIT") == "1")
                Command.CreateCommand("QUIT").RunInPdms();
        }

        private static int DelayMs()
        {
            int ms;
            return int.TryParse(Environment.GetEnvironmentVariable("GENMODEL_E3D_DELAY_MS"),
                NumberStyles.Integer, CultureInfo.InvariantCulture, out ms) && ms > 0 ? ms : 30000;
        }

        private static void Log(string line)
        {
            string path = Environment.GetEnvironmentVariable("GENMODEL_E3D_LOG");
            if (string.IsNullOrEmpty(path)) path = @"D:\work\plant-code\old\gen-model\output\e3d_incremental_test.log";
            File.AppendAllText(path, DateTime.Now.ToString("O", CultureInfo.InvariantCulture) + " " + line + "\r\n",
                new UTF8Encoding(false));
        }
    }
}
