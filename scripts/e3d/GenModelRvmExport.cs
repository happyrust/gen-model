// 无人值守 RVM 基准导出：E3D CAF addin。
//
// 为什么要插件：向运行中的 E3D 会话发一条命令，实测只有 CAF addin 这一条路走得通。
// 控制台注入（AttachConsole + WriteConsoleInput）写得进 CONIN$ 但 des.exe 不消费；
// PDMS_NOGRAPHICS 下 stdin 被 core.dll 忽略；AVEVA_DESIGN_ENTRYMACRO 在直接 des.exe
// 启动时不生效；UIAutomation 扫不到命令窗口的输入框。
//
// 命令序列取自 E3D 自带的 PMLUI/intf/review/mexpmain —— 那是 Design Export 表单点
// Run 时动态生成并执行的临时宏，把它写出来的原生命令固化下来即可，无需导出模板。
// 驱动名 expdri.so 取自 %AVEVA_DESIGN_DFLTS%/export/driver-config 中唯一的 Review 条目。
//
// Build（x86，AVEVA 程序集是 32 位）:
//   csc /platform:x86 /target:library /out:GenModelRvmExport.dll GenModelRvmExport.cs
//       /r:"<E3D>\Aveva.ApplicationFramework.dll" /r:"<E3D>\Aveva.Core.Utilities.dll"
//       /r:"<E3D>\PMLNet.dll" /r:System.Windows.Forms.dll
//
// 注册：把 GenModelRvmExport 加进 <E3D>\DesignAddins.xml 的 ArrayOfString。
//
// 两个入口：
//   1. RvmExportAddin —— CAF addin，启动后延迟若干毫秒自动跑一次，不需要命令窗口。
//   2. RVMEXPORT —— PML.NET 可调用，手工驱动同一段逻辑：
//        import 'GenModelRvmExport'
//        !e = object RVMEXPORT()
//        !s = !e.Export('/C-IY-1R330-B', 'D:/path/out.rvm')
//
// 环境变量（addin 模式）：
//   GENMODEL_RVM_ELEMENT   要导出的元素，默认 /C-IY-1R330-B
//   GENMODEL_RVM_OUT       输出 .rvm 路径
//   GENMODEL_RVM_LOG       日志路径
//   GENMODEL_RVM_DELAY_MS  首个 Idle 之后再等多久才执行，默认 8000
//   GENMODEL_RVM_QUIT      置 1 则导出后退出 E3D，实现真正无人值守
//   GENMODEL_RVM_INSU      保温表现，默认 "off"
//   GENMODEL_RVM_OBST      障碍表现，默认 "off"
//   GENMODEL_RVM_LEVEL     显示层级，默认 "6"（对齐 rs-core LEVEL_VISBLE）
//   GENMODEL_ATT_OUT       输出 .att 路径，置空则跳过属性导出

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Windows.Forms;
using Aveva.ApplicationFramework;
using Aveva.Core.PMLNet;
using Aveva.Core.Utilities.CommandLine;

[assembly: PMLNetCallable()]

namespace GenModel.E3D.Rvm
{
    internal static class RvmExportCore
    {
        internal const string DefaultElement = "/C-IY-1R330-B";
        internal const string DefaultOutput =
            @"D:\work\plant-code\old\gen-model\test_data\rvm\C-IY-1R330-B.rvm";

        /// 导出口径。默认使用对拍需要的窄口径（无保温、无障碍、显示层级 6）。
        ///
        /// 口径直接决定对拍是否可信：全导会把 OBST≠0 的障碍/预留体和 0 级几何一并写进 RVM，
        /// 而生成侧按 TUFL / LEVE 把它们排除掉，两侧比包围盒就会整片误报。要拿到可判定的
        /// 基准，就得让 RVM 侧只留生成侧同样会产出的那部分：insu/obst 关掉、层级设成 6
        /// （= rs-core 的 LEVEL_VISBLE，`is_visible_by_level(None)` 用的就是它）。
        ///
        /// repre lev / insu / obst 的写法取自 PMLUI/intf/review/mexpmain。
        internal static string[] BuildCommands(string element, string outPath)
        {
            string file = outPath.Replace('\\', '/');
            string insu = Env("GENMODEL_RVM_INSU", "off");
            string obst = Env("GENMODEL_RVM_OBST", "off");
            string level = Env("GENMODEL_RVM_LEVEL", "6");

            List<string> cmds = new List<string>();
            if (!string.IsNullOrEmpty(level))
            {
                cmds.Add("repre lev " + level);
                cmds.Add("repre lev pipe " + level);
                cmds.Add("repre lev nozz " + level);
                cmds.Add("repre lev struc " + level);
            }
            cmds.Add("repre insu " + insu);
            cmds.Add("repre obst " + obst);
            cmds.Add("repre tube on");
            cmds.Add("export implied tube into separate containers");
            cmds.Add("export system /expdri.so");
            cmds.Add("export file \"" + file + "\"");
            cmds.Add("export filenote 'gen-model RVM baseline insu=" + insu + " obst=" + obst
                     + " lev=" + (level == "" ? "default" : level) + "'");
            cmds.Add("export holes on");
            cmds.Add("export autocolour displayexport on");
            cmds.Add("export repr on");
            cmds.Add("export " + element);
            cmds.Add("export finish");
            return cmds.ToArray();
        }

        internal static string Env(string name, string fallback)
        {
            string v = Environment.GetEnvironmentVariable(name);
            return string.IsNullOrEmpty(v) ? fallback : v;
        }

        private static string TemporaryOutputPath(string finalPath)
        {
            string full = Path.GetFullPath(finalPath);
            string dir = Path.GetDirectoryName(full);
            string stem = Path.GetFileNameWithoutExtension(full);
            string ext = Path.GetExtension(full);
            return Path.Combine(dir, stem + "." + Guid.NewGuid().ToString("N") + ".tmp" + ext);
        }

        private static void Publish(string temporaryPath, string finalPath)
        {
            if (File.Exists(finalPath))
            {
                File.Replace(temporaryPath, finalPath, null);
            }
            else
            {
                File.Move(temporaryPath, finalPath);
            }
        }

        private static void RestorePublished(string finalPath, string backupPath, bool existed)
        {
            if (!existed)
            {
                if (File.Exists(finalPath)) File.Delete(finalPath);
                return;
            }
            if (!File.Exists(backupPath)) return;
            if (File.Exists(finalPath)) File.Replace(backupPath, finalPath, null);
            else File.Move(backupPath, finalPath);
        }

        private static void PublishPair(
            string stagedRvm,
            string finalRvm,
            string stagedAtt,
            string finalAtt,
            Action<string> journal)
        {
            bool rvmExisted = File.Exists(finalRvm);
            bool attExisted = File.Exists(finalAtt);
            string rvmBackup = rvmExisted ? TemporaryOutputPath(finalRvm) : null;
            string attBackup = attExisted ? TemporaryOutputPath(finalAtt) : null;
            bool rvmPublished = false;
            bool attPublished = false;

            try
            {
                if (rvmExisted) File.Replace(stagedRvm, finalRvm, rvmBackup);
                else File.Move(stagedRvm, finalRvm);
                rvmPublished = true;

                if (attExisted) File.Replace(stagedAtt, finalAtt, attBackup);
                else File.Move(stagedAtt, finalAtt);
                attPublished = true;
            }
            catch
            {
                try
                {
                    if (attPublished) RestorePublished(finalAtt, attBackup, attExisted);
                }
                finally
                {
                    if (rvmPublished) RestorePublished(finalRvm, rvmBackup, rvmExisted);
                }
                throw;
            }
            if (rvmBackup != null) DeleteTemporary(rvmBackup, journal, "rvm backup");
            if (attBackup != null) DeleteTemporary(attBackup, journal, "att backup");
            // ponytail: 两次文件替换之间仍有进程崩溃窗口；需要崩溃一致性时改为清单指针切换。
        }

        private static void DeleteTemporary(string path, Action<string> journal, string label)
        {
            try
            {
                if (File.Exists(path)) File.Delete(path);
            }
            catch (Exception ex)
            {
                journal(label + ": could not delete temporary output: " + ex.Message);
            }
        }

        internal static string Run(string element, string outPath, Action<string> journal)
        {
            if (string.IsNullOrEmpty(element)) element = DefaultElement;
            if (string.IsNullOrEmpty(outPath)) outPath = DefaultOutput;

            outPath = Path.GetFullPath(outPath);

            string dir = Path.GetDirectoryName(outPath);
            if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir)) Directory.CreateDirectory(dir);
            // 先写同目录临时文件，全部命令成功且文件非空后再原子替换；失败时旧基准保留。
            string temporaryPath = TemporaryOutputPath(outPath);

            Diagnose(journal);

            string[] commands = BuildCommands(element, temporaryPath);
            int failed = 0;
            for (int i = 0; i < commands.Length; i++)
            {
                string line = commands[i];
                string outcome = Submit(line);
                journal(line + "   -> " + outcome);
                if (outcome.StartsWith("ERR", StringComparison.Ordinal)) failed++;
            }

            bool produced = File.Exists(temporaryPath);
            long size = produced ? new FileInfo(temporaryPath).Length : 0L;
            bool publishable = failed == 0 && size > 0;
            if (publishable) Publish(temporaryPath, outPath);
            else DeleteTemporary(temporaryPath, journal, "rvm");
            string verdict = (publishable ? "OK" : "FAILED")
                + " failedCommands=" + failed.ToString(CultureInfo.InvariantCulture)
                + " bytes=" + size.ToString(CultureInfo.InvariantCulture)
                + " -> " + outPath;
            journal(verdict);
            return verdict;
        }

        /// ATT 属性导出。
        ///
        /// 与几何不同，属性没有原生命令：`cdxattdump` 是 PMLLIB 里的一张表单，
        /// 真正干活的 `mattdump()` 自己 openfile + 逐属性 writefile，参数全部从
        /// 表单字段读。所以只能把表单调出来、填字段、再调方法。
        ///
        /// 两个坑：输出文件已存在时 mattdump 会弹 alert.confirm 卡死无人值守流程，
        /// 所以写唯一临时文件再发布；表单未加载时 `!!cdxAttDump` 未定义，所以先 show 再用。
        internal static string RunAttDump(string element, string attPath, Action<string> journal)
        {
            if (string.IsNullOrEmpty(attPath)) return "SKIPPED";
            if (string.IsNullOrEmpty(element)) element = DefaultElement;

            attPath = Path.GetFullPath(attPath);

            string dir = Path.GetDirectoryName(attPath);
            if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir)) Directory.CreateDirectory(dir);
            string temporaryPath = TemporaryOutputPath(attPath);

            string file = temporaryPath.Replace('\\', '/');
            string[] commands = new string[]
            {
                element,
                "show !!cdxAttDump at xr 0.5 yr 0.5",
                "!!cdxAttDump.fNam.val = '" + file + "'",
                "!!cdxAttDump.ce.val = true",
                "!!cdxAttDump.unsets.val = true",
                "!!cdxAttDump.tube.val = true",
                "!!cdxAttDump.mattdump()",
                "!!cdxAttDump.hide()"
            };

            int failed = 0;
            for (int i = 0; i < commands.Length; i++)
            {
                string outcome = Submit(commands[i]);
                journal("att: " + commands[i] + "   -> " + outcome);
                if (outcome.StartsWith("ERR", StringComparison.Ordinal)) failed++;
            }

            bool produced = File.Exists(temporaryPath);
            long size = produced ? new FileInfo(temporaryPath).Length : 0L;
            bool publishable = failed == 0 && size > 0;
            if (publishable) Publish(temporaryPath, attPath);
            else DeleteTemporary(temporaryPath, journal, "att");
            string verdict = "ATT " + (publishable ? "OK" : "FAILED")
                + " failedCommands=" + failed.ToString(CultureInfo.InvariantCulture)
                + " bytes=" + size.ToString(CultureInfo.InvariantCulture)
                + " -> " + attPath;
            journal(verdict);
            return verdict;
        }

        internal static string RunPair(
            string element,
            string rvmPath,
            string attPath,
            Action<string> journal)
        {
            rvmPath = Path.GetFullPath(rvmPath);
            attPath = Path.GetFullPath(attPath);
            if (string.Equals(rvmPath, attPath, StringComparison.OrdinalIgnoreCase))
            {
                throw new ArgumentException("RVM and ATT outputs must use different paths");
            }

            string stagedRvm = TemporaryOutputPath(rvmPath);
            string stagedAtt = TemporaryOutputPath(attPath);
            try
            {
                string rvmVerdict = Run(element, stagedRvm, journal);
                if (!rvmVerdict.StartsWith("OK", StringComparison.Ordinal))
                {
                    return "PAIR FAILED: " + rvmVerdict;
                }

                string attVerdict = RunAttDump(element, stagedAtt, journal);
                if (!attVerdict.StartsWith("ATT OK", StringComparison.Ordinal))
                {
                    return "PAIR FAILED: " + attVerdict;
                }

                PublishPair(stagedRvm, rvmPath, stagedAtt, attPath, journal);
                string verdict = "PAIR OK -> " + rvmPath + " + " + attPath;
                journal(verdict);
                return verdict;
            }
            finally
            {
                DeleteTemporary(stagedRvm, journal, "staged rvm");
                DeleteTemporary(stagedAtt, journal, "staged att");
            }
        }

        /// 所有命令都回同一个错误码时，问题多半在会话上下文而不在命令本身，
        /// 所以先把 MDB / 当前元素 / 四种提交方式各自的结果记下来。
        private static void Diagnose(Action<string> journal)
        {
            try
            {
                Aveva.Core.Database.MDB mdb = Aveva.Core.Database.MDB.CurrentMDB;
                journal("diag mdb=" + (mdb == null ? "<null>" : mdb.Name));
                if (mdb != null)
                {
                    Aveva.Core.Database.Db[] dbs = mdb.GetDBArray();
                    journal("diag dbs=" + (dbs == null ? "<null>" : dbs.Length.ToString(CultureInfo.InvariantCulture)));
                }
            }
            catch (Exception ex)
            {
                journal("diag mdb threw " + ex.GetType().Name + ": " + ex.Message);
            }

            try
            {
                Aveva.Core.Database.DbElement ce = Aveva.Core.Database.CurrentElement.Element;
                journal("diag ce=" + (ce == null || !ce.IsValid ? "<invalid>" : ce.GetAsString(Aveva.Core.Database.DbAttributeInstance.NAME)));
            }
            catch (Exception ex)
            {
                journal("diag ce threw " + ex.GetType().Name + ": " + ex.Message);
            }

            journal("diag Command.Enabled=" + Command.Enabled.ToString());

            foreach (string probe in new string[] { "$P GENMODEL-PROBE", "VAR !GMPROBE 'x'" })
            {
                try
                {
                    Command c = Command.CreateCommand(probe);
                    string r = "";
                    try { r += "RunInPdms=" + c.RunInPdms(); } catch (Exception ex) { r += "RunInPdms threw " + ex.GetType().Name; }
                    try { r += " Run=" + c.Run(); } catch (Exception ex) { r += " Run threw " + ex.GetType().Name; }
                    try { r += " InScope=" + c.RunInCurrentScope(); } catch (Exception ex) { r += " InScope threw " + ex.GetType().Name; }
                    try { r += " err=" + Describe(c); } catch { }
                    journal("diag probe [" + probe + "] " + r);
                }
                catch (Exception ex)
                {
                    journal("diag probe [" + probe + "] threw " + ex.GetType().Name + ": " + ex.Message);
                }
            }

            try
            {
                Command.RunPMLCommandInPDMS("$P GENMODEL-PMLPROBE");
                journal("diag RunPMLCommandInPDMS ok");
            }
            catch (Exception ex)
            {
                journal("diag RunPMLCommandInPDMS threw " + ex.GetType().Name + ": " + ex.Message);
            }
        }

        /// RunInPdms 走 PDMS 命令上下文，是原生 export/repre 命令该走的路；
        /// 个别命令在当前作用域下才认，所以失败时退回 Run 再试一次。
        private static string Submit(string commandText)
        {
            try
            {
                Command command = Command.CreateCommand(commandText);
                bool ok;
                try { ok = command.RunInPdms(); }
                catch { ok = false; }
                if (!ok)
                {
                    try { ok = command.Run(); }
                    catch { }
                }
                if (ok) return "ok";
                return "ERR " + Describe(command);
            }
            catch (Exception ex)
            {
                return "ERR " + ex.GetType().Name + ": " + ex.Message;
            }
        }

        private static string Describe(Command command)
        {
            try
            {
                Aveva.Core.Utilities.Messaging.PdmsMessage m = command.Error;
                if (m != null)
                {
                    return "pdms " + m.ModuleNumber.ToString(CultureInfo.InvariantCulture)
                         + "/" + m.MessageNumber.ToString(CultureInfo.InvariantCulture);
                }
            }
            catch
            {
            }
            try
            {
                string r = command.Result;
                if (!string.IsNullOrEmpty(r)) return r;
            }
            catch
            {
            }
            return "no detail";
        }
    }

    /// PML.NET 手工入口。类名即 PML 对象名，保持短。
    [PMLNetCallable()]
    public class RvmExport
    {
        [PMLNetCallable()]
        public RvmExport()
        {
        }

        [PMLNetCallable()]
        public void Assign(RvmExport other)
        {
        }

        [PMLNetCallable()]
        public string Export(string element, string outPath)
        {
            try
            {
                StringBuilder log = new StringBuilder();
                string verdict = RvmExportCore.Run(element, outPath, delegate(string s) { log.AppendLine(s); });
                return verdict;
            }
            catch (Exception ex)
            {
                return "ERROR " + ex.GetType().Name + ": " + ex.Message;
            }
        }
    }

    public class RvmExportAddin : IAddin
    {
        private Timer _timer;

        public string Name
        {
            get { return "GenModel RVM Baseline Export"; }
        }

        public string Description
        {
            get { return "Unattended RVM export of one element, for gen-model baseline comparison."; }
        }

        /// 插件一旦注册进 DesignAddins.xml 就会在每个 DESIGN 会话里加载，
        /// 所以必须显式设了 GENMODEL_RVM_ELEMENT 才真正干活，
        /// 免得平时正常开 E3D 时莫名其妙跑一次导出。
        public void Start(ServiceManager serviceManager)
        {
            string element = Environment.GetEnvironmentVariable("GENMODEL_RVM_ELEMENT");
            if (string.IsNullOrEmpty(element))
            {
                return;
            }
            Journal("addin-start");
            Application.Idle += OnIdle;
        }

        public void Stop()
        {
            if (_timer != null)
            {
                _timer.Stop();
                _timer.Dispose();
                _timer = null;
            }
        }

        /// 首个 Idle 说明模块已经起来，但 MDB / DESIGN 上下文未必就绪，
        /// 所以再挂一个一次性定时器把导出推后一点。
        private void OnIdle(object sender, EventArgs e)
        {
            Application.Idle -= OnIdle;
            Journal("idle-reached, arming timer");
            _timer = new Timer();
            _timer.Interval = DelayMs();
            _timer.Tick += OnTick;
            _timer.Start();
        }

        private void OnTick(object sender, EventArgs e)
        {
            _timer.Stop();
            Environment.ExitCode = 0;
            try
            {
                string element = Env("GENMODEL_RVM_ELEMENT", RvmExportCore.DefaultElement);
                string outPath = Env("GENMODEL_RVM_OUT", RvmExportCore.DefaultOutput);
                string attPath = Env("GENMODEL_ATT_OUT", "");
                string verdict;
                if (!string.IsNullOrEmpty(attPath))
                {
                    Journal("exporting pair " + element + " -> " + outPath + " + " + attPath);
                    verdict = RvmExportCore.RunPair(element, outPath, attPath, Journal);
                }
                else
                {
                    Journal("exporting " + element + " -> " + outPath);
                    verdict = RvmExportCore.Run(element, outPath, Journal);
                }
                if (!verdict.StartsWith("OK", StringComparison.Ordinal)
                    && !verdict.StartsWith("PAIR OK", StringComparison.Ordinal))
                {
                    throw new InvalidOperationException(verdict);
                }
            }
            catch (Exception ex)
            {
                Environment.ExitCode = 1;
                Journal("THREW " + ex.GetType().Name + ": " + ex.Message);
            }

            if (Env("GENMODEL_RVM_QUIT", "") == "1")
            {
                Journal("quitting session");
                try { Command.CreateCommand("QUIT").RunInPdms(); }
                catch (Exception ex) { Journal("quit failed: " + ex.Message); }
            }
        }

        private static int DelayMs()
        {
            int ms;
            string raw = Environment.GetEnvironmentVariable("GENMODEL_RVM_DELAY_MS");
            if (!string.IsNullOrEmpty(raw)
                && int.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out ms)
                && ms > 0)
            {
                return ms;
            }
            return 8000;
        }

        private static string Env(string name, string fallback)
        {
            string v = Environment.GetEnvironmentVariable(name);
            return string.IsNullOrEmpty(v) ? fallback : v;
        }

        private static void Journal(string line)
        {
            try
            {
                string log = Environment.GetEnvironmentVariable("GENMODEL_RVM_LOG");
                if (string.IsNullOrEmpty(log))
                    log = @"D:\work\plant-code\old\gen-model\output\rvm_export_addin.log";
                string dir = Path.GetDirectoryName(log);
                if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir)) Directory.CreateDirectory(dir);
                File.AppendAllText(log,
                    DateTime.Now.ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "  " + line + "\r\n",
                    new UTF8Encoding(false));
            }
            catch
            {
            }
        }
    }
}
