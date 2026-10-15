// 仅用于 windows_auth_check.ps1 -VerifyKeygenAbort：在真实 fixture 首次派生窗口
// 创建并挂起已有 ssh-keygen，允许它的父进程先退出，验证 Job 仍能清理此真实子进程。
// 不生成密钥、不实现 SSH、不替代成功链的 ssh-keygen；产物只放本轮临时目录。
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

internal static class KeygenAbortProbe {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct StartupInfo {
        public uint Size;
        public string Reserved, Desktop, Title;
        public uint X, Y, XSize, YSize, XChars, YChars, Fill, Flags;
        public ushort Show, ReservedSize;
        public IntPtr ReservedData, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo {
        public IntPtr Process, Thread;
        public uint ProcessId, ThreadId;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcessW(string application, StringBuilder command,
        IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint flags,
        IntPtr environment, string directory, ref StartupInfo startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(IntPtr process, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    private static string Required(string name) {
        string value = Environment.GetEnvironmentVariable(name);
        if (String.IsNullOrEmpty(value)) throw new InvalidOperationException("缺少本轮变量：" + name);
        return value;
    }

    private static string Quote(string value) {
        var result = new StringBuilder("\"");
        int backslashes = 0;
        foreach (char character in value) {
            if (character == '\\') { backslashes++; continue; }
            result.Append('\\', backslashes * (character == '"' ? 2 : 1));
            if (character == '"') result.Append('\\');
            result.Append(character);
            backslashes = 0;
        }
        result.Append('\\', backslashes * 2).Append('"');
        return result.ToString();
    }

    private static int Main(string[] args) {
        ProcessInfo child = new ProcessInfo();
        try {
            string run = Path.GetFullPath(Required("GUOSH_AUTH_RUN"));
            string runId = Required("GUOSH_AUTH_RUN_ID");
            string fixture = Path.Combine(run, "server");
            string realKeygen = Path.GetFullPath(Required("GUOSH_AUTH_REAL_KEYGEN"));
            if (!Regex.IsMatch(runId, "^[a-f0-9]{32}$") ||
                Path.GetFileName(run) != "guossh-auth-" + runId ||
                File.ReadAllText(Path.Combine(run, ".owner")) != runId ||
                Required("GUOSH_DEMO_DATA") != fixture || !File.Exists(realKeygen) ||
                args.Length != 7 || args[0] != "-t" || args[1] != "ed25519" ||
                args[2] != "-N" || args[3] != "" || args[4] != "-q" || args[5] != "-f" ||
                Path.GetFullPath(args[6]) != Path.Combine(fixture, "hostkey") ||
                File.Exists(args[6])) {
                throw new InvalidOperationException("首次密钥派生探针所有权或参数不符");
            }
            var command = new StringBuilder(Quote(realKeygen));
            foreach (string argument in args) command.Append(' ').Append(Quote(argument));
            var startup = new StartupInfo();
            startup.Size = (uint)Marshal.SizeOf(typeof(StartupInfo));
            // CREATE_SUSPENDED；不允许 breakaway，真实 ssh-keygen 继承 fixture 的 Job。
            if (!CreateProcessW(realKeygen, command, IntPtr.Zero, IntPtr.Zero, false, 4,
                IntPtr.Zero, fixture, ref startup, out child)) throw new Win32Exception();
            string report = Path.Combine(run, "keygen.pid");
            File.WriteAllText(report + ".new", child.ProcessId.ToString());
            File.Move(report + ".new", report);
            using (var release = EventWaitHandle.OpenExisting(
                "Local\\GuoSSHell-auth-" + runId + "-keygen-parent-exit")) {
                if (!release.WaitOne(30000)) throw new TimeoutException("派生父进程退出门超时");
            }
            // 故意先退出 shim；无 hostkey 的 demo_server 随后退出，真实挂起子进程留给 Job。
            return 0;
        } catch (Exception error) {
            Console.Error.WriteLine("[keygen-probe] " + error.Message);
            if (child.Process != IntPtr.Zero) {
                if (!TerminateProcess(child.Process, 1) || WaitForSingleObject(child.Process, 15000) != 0)
                    throw new Win32Exception();
            }
            return 1;
        } finally {
            if (child.Thread != IntPtr.Zero && !CloseHandle(child.Thread)) throw new Win32Exception();
            if (child.Process != IntPtr.Zero && !CloseHandle(child.Process)) throw new Win32Exception();
        }
    }
}
