// One-shot exporter: dumps every element type's ordered implicit-attribute layout
// from the live E3D dictionary so gen-model can parse element binaries offline.
//
// Build (x86, AVEVA assemblies are 32-bit):
//   csc /platform:x86 /target:library /out:GenModelNounLayout.dll NounLayoutExport.cs
//       /r:"<E3D>\Aveva.Core.Database.dll" /r:"<E3D>\PMLNet.dll"
//       /r:"<E3D>\Aveva.ApplicationFramework.dll" /r:System.Windows.Forms.dll
//
// Two entry points, because AVEVA_DESIGN_ENTRYMACRO is not honoured on a direct
// des.exe launch (the literal appears in no shipped binary but Startup.dll):
//   1. NounLayoutAddin  - a CAF addin, registered by adding "GenModelNounLayout"
//      to DesignAddins.xml. Fires by itself on the first idle after startup, so it
//      needs neither the appware start macro nor a command window.
//   2. NounLayoutExport - PML.NET callable, for driving the same export by hand:
//        import 'GenModelNounLayout'
//        !e = object NOUNLAYOUTEXPORT()
//        !s = !e.export('D:/work/plant-code/old/gen-model/output/noun_layout.json')

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;
using Aveva.ApplicationFramework;
using Aveva.Core.Database;
using Aveva.Core.PMLNet;

// Without this assembly-level marker PMLNet rejects the whole DLL at `import`
// time with "is not PML.NET callable", no matter how the classes are attributed.
[assembly: PMLNetCallable()]

namespace GenModel.E3D
{
    [PMLNetCallable()]
    public class NounLayoutExport
    {
        [PMLNetCallable()]
        public NounLayoutExport()
        {
        }

        [PMLNetCallable()]
        public void Assign(NounLayoutExport other)
        {
        }

        [PMLNetCallable()]
        public string Export(string path)
        {
            try
            {
                return Run(path);
            }
            catch (Exception ex)
            {
                return "ERROR " + ex.GetType().Name + ": " + ex.Message;
            }
        }

        [PMLNetCallable()]
        public string ExportSlots(string path)
        {
            return ExportSlotsN(path, "");
        }

        /// Same, with an explicit element budget. Driving this from the console of a
        /// session someone else is using means the walk has to be kept short enough
        /// not to freeze their UI, so the budget is worth being able to dial down.
        [PMLNetCallable()]
        public string ExportSlotsN(string path, string budget)
        {
            try
            {
                int n;
                if (!int.TryParse(budget, NumberStyles.Integer, CultureInfo.InvariantCulture, out n) || n <= 0)
                    n = DabSlotExport.DefaultElementBudget;
                return DabSlotExport.Run(path, n);
            }
            catch (Exception ex)
            {
                return "ERROR " + ex.GetType().Name + ": " + ex.Message;
            }
        }

        // Every distinct attribute met while walking the types, so the 57-field
        // dictionary dump costs one entry per attribute instead of one per
        // (noun, attribute) pair.
        private static readonly Dictionary<int, DbAttribute> Seen = new Dictionary<int, DbAttribute>();

        private static string Run(string path)
        {
            Seen.Clear();
            List<DbElementType> types = new List<DbElementType>();
            types.AddRange(DbElementType.GetAllElementTypes());
            try
            {
                DbElementType[] udets = DbElementType.GetAllUdets();
                if (udets != null) types.AddRange(udets);
            }
            catch
            {
            }

            StringBuilder sb = new StringBuilder(1 << 22);
            sb.Append("[\n");
            int nouns = 0;
            int attrs = 0;
            for (int i = 0; i < types.Count; i++)
            {
                DbElementType t = types[i];
                if (t == null || !t.IsValid) continue;
                if (nouns > 0) sb.Append(",\n");
                nouns += WriteType(sb, t, ref attrs);
            }
            sb.Append("\n]\n");

            string dir = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir)) Directory.CreateDirectory(dir);
            File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));

            string fieldsPath = Path.Combine(dir ?? ".", "noun_attr_fields.json");
            int fieldCount = WriteAttributeFields(fieldsPath);

            return "OK nouns=" + nouns.ToString(CultureInfo.InvariantCulture)
                 + " attrs=" + attrs.ToString(CultureInfo.InvariantCulture)
                 + " distinct=" + Seen.Count.ToString(CultureInfo.InvariantCulture)
                 + " fields=" + fieldCount.ToString(CultureInfo.InvariantCulture)
                 + " -> " + path + " + " + fieldsPath;
        }

        /// Dump every DbAttributeField of every distinct attribute. The getter that
        /// suits a given field is not discoverable up front, so all three are tried
        /// and whichever return a value are emitted for offline sorting out.
        private static int WriteAttributeFields(string path)
        {
            Array fields;
            try
            {
                fields = Enum.GetValues(typeof(DbAttributeField));
            }
            catch
            {
                return 0;
            }

            StringBuilder sb = new StringBuilder(1 << 22);
            sb.Append("{\n");
            int written = 0;
            bool firstAttr = true;
            foreach (KeyValuePair<int, DbAttribute> kv in Seen)
            {
                DbAttribute a = kv.Value;
                if (!firstAttr) sb.Append(",\n");
                firstAttr = false;
                sb.Append(" \"").Append(kv.Key.ToString(CultureInfo.InvariantCulture)).Append("\":{");
                sb.Append("\"name\":\"").Append(Esc(SafeAttrName(a))).Append("\",\"f\":{");
                bool firstField = true;
                foreach (object f in fields)
                {
                    DbAttributeField fld = (DbAttributeField)f;
                    string parts = ReadField(a, fld);
                    if (parts == null) continue;
                    if (!firstField) sb.Append(",");
                    firstField = false;
                    sb.Append("\"").Append(Esc(fld.ToString())).Append("\":").Append(parts);
                    written++;
                }
                sb.Append("}}");
            }
            sb.Append("\n}\n");
            File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
            return written;
        }

        private static string ReadField(DbAttribute a, DbAttributeField f)
        {
            StringBuilder o = new StringBuilder(48);
            o.Append("{");
            bool any = false;
            try
            {
                int i = a.GetInteger(f);
                o.Append("\"i\":").Append(i.ToString(CultureInfo.InvariantCulture));
                any = true;
            }
            catch
            {
            }
            try
            {
                bool b = a.GetBool(f);
                if (any) o.Append(",");
                o.Append("\"b\":").Append(SafeBool(b));
                any = true;
            }
            catch
            {
            }
            try
            {
                string s = a.GetString(f);
                if (!string.IsNullOrEmpty(s))
                {
                    if (any) o.Append(",");
                    o.Append("\"s\":\"").Append(Esc(s)).Append("\"");
                    any = true;
                }
            }
            catch
            {
            }
            if (!any) return null;
            o.Append("}");
            return o.ToString();
        }

        private static string SafeAttrName(DbAttribute a)
        {
            try { return a.Name; } catch { return ""; }
        }

        private static int WriteType(StringBuilder sb, DbElementType t, ref int attrCount)
        {
            sb.Append(" {\"noun\":\"").Append(Esc(t.Name)).Append("\"");
            sb.Append(",\"short\":\"").Append(Esc(SafeShortName(t))).Append("\"");
            sb.Append(",\"hash\":").Append(SafeNounHash(t).ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"base\":\"").Append(Esc(SafeName(SafeBase(t)))).Append("\"");
            sb.Append(",\"hard\":\"").Append(Esc(SafeName(SafeHard(t)))).Append("\"");
            sb.Append(",\"isUdet\":").Append(SafeBool(SafeIsUdet(t)));
            sb.Append(",\"isPseudo\":").Append(SafeBool(t.IsPseudo));
            sb.Append(",\"isWorld\":").Append(SafeBool(t.IsWorld));
            sb.Append(",\"isPrimary\":").Append(SafeBool(t.IsPrimary));
            sb.Append(",\"visible\":").Append(SafeBool(t.Visible));
            sb.Append(",\"dbTypes\":[");
            try
            {
                int[] dbs = t.DatabaseTypes();
                if (dbs != null)
                    for (int k = 0; k < dbs.Length; k++)
                    {
                        if (k > 0) sb.Append(",");
                        sb.Append(dbs[k].ToString(CultureInfo.InvariantCulture));
                    }
            }
            catch
            {
            }
            sb.Append("]");

            sb.Append(",\"attrs\":[");
            DbAttribute[] list = null;
            try
            {
                list = t.SystemAttributes();
            }
            catch
            {
            }
            if (list != null)
            {
                for (int j = 0; j < list.Length; j++)
                {
                    DbAttribute a = list[j];
                    if (a == null) continue;
                    if (j > 0) sb.Append(",");
                    int trueSize = -1;
                    try
                    {
                        trueSize = a.TrueSizE(t);
                    }
                    catch
                    {
                    }
                    sb.Append("\n  {\"name\":\"").Append(Esc(a.Name)).Append("\"");
                    sb.Append(",\"hash\":").Append(a.HashValue.ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"type\":").Append(((int)a.Type).ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"typeName\":\"").Append(Esc(a.Type.ToString())).Append("\"");
                    sb.Append(",\"isArray\":").Append(SafeBool(a.IsArray));
                    sb.Append(",\"maxSize\":").Append(SafeInt(a.MaximumSize));
                    sb.Append(",\"trueSize\":").Append(trueSize.ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"isUda\":").Append(SafeBool(a.IsUDA));
                    sb.Append(",\"isPseudo\":").Append(SafeBool(a.IsPseudo));
                    // all three are per (noun, attribute), so they belong here rather
                    // than in the shared attribute dictionary
                    sb.Append(",\"hidden\":").Append(SafeBool(SafeHidden(a, t)));
                    sb.Append(",\"noClaim\":").Append(SafeBool(SafeNoClaim(a, t)));
                    // DB_Attribute::defaultExp(DB_Noun*) suggests an attribute backed by
                    // a default expression is computed rather than stored, which would
                    // be exactly the "does it occupy an implicit slot" predicate.
                    sb.Append(",\"def\":\"").Append(Esc(DefaultProbe(a, t))).Append("\"");
                    sb.Append("}");
                    if (!Seen.ContainsKey(a.HashValue)) Seen[a.HashValue] = a;
                    attrCount++;
                }
                sb.Append("\n ");
            }
            sb.Append("]}");
            return 1;
        }

        private static DbElementType SafeBase(DbElementType t)
        {
            try { return t.BaseType; } catch { return null; }
        }

        private static string SafeShortName(DbElementType t)
        {
            try { return t.ShortName; } catch { return ""; }
        }

        private static int SafeNounHash(DbElementType t)
        {
            try { return t.GetHashCode(); } catch { return 0; }
        }

        private static DbElementType SafeHard(DbElementType t)
        {
            try { return t.HardType; } catch { return null; }
        }

        private static bool SafeIsUdet(DbElementType t)
        {
            try { return t.IsUDET(); } catch { return false; }
        }

        private static bool SafeHidden(DbAttribute a, DbElementType t)
        {
            try { return a.HiddenByType(t); } catch { return false; }
        }

        private static bool SafeNoClaim(DbAttribute a, DbElementType t)
        {
            try { return a.NoClaim(t); } catch { return false; }
        }

        /// A null PdmsMessage means the default was retrieved; a non-null one carries
        /// the refusal code, which is itself worth grouping on offline.
        private static string DefaultProbe(DbAttribute a, DbElementType t)
        {
            try
            {
                int iv;
                Aveva.Core.Utilities.Messaging.PdmsMessage m = a.GetDefault(t, out iv);
                if (m == null) return "i:" + iv.ToString(CultureInfo.InvariantCulture);
                return "e" + m.ModuleNumber.ToString(CultureInfo.InvariantCulture)
                     + "/" + m.MessageNumber.ToString(CultureInfo.InvariantCulture);
            }
            catch (Exception ex)
            {
                return "x:" + ex.GetType().Name;
            }
        }

        private static string SafeName(DbElementType t)
        {
            if (t == null) return "";
            try { return t.IsValid ? t.Name : ""; } catch { return ""; }
        }

        private static string SafeInt(int v)
        {
            return v.ToString(CultureInfo.InvariantCulture);
        }

        private static string SafeBool(bool b)
        {
            return b ? "true" : "false";
        }

        private static string Esc(string s)
        {
            if (string.IsNullOrEmpty(s)) return "";
            StringBuilder o = new StringBuilder(s.Length + 8);
            for (int i = 0; i < s.Length; i++)
            {
                char c = s[i];
                if (c == '"' || c == '\\') { o.Append('\\').Append(c); }
                else if (c < ' ') { o.Append(' '); }
                else { o.Append(c); }
            }
            return o.ToString();
        }
    }

    /// Reads the dabacon per-element-type attribute descriptor table straight out of
    /// core.dll's memory. This is the authoritative answer to "which attributes claim
    /// a slot in the implicit block", which no dictionary field carries.
    ///
    /// Reversed from sub_5AB5920 / sub_5AB5BC0 / sub_5AB5600:
    ///
    ///   P    = *(core.dll + 0x18E4024)   dabacon DB stack
    ///   idx  = *(P + 8)                  index of the current context
    ///   CTX  = P + 60*idx + 16           current context record
    ///   TBL  = *CTX                      attribute table of the CURRENT element type
    ///     TBL + 4  = element type hash
    ///     TBL + 36 = entry count
    ///     TBL + 56 = first entry; entry[0] = attribute key, entry[1] = own dword length
    ///     entry + 20, or + 32 when the element record's flag word has 0x20000000:
    ///        low 20 bits = implicit-block word offset (0 = attribute is in the
    ///        explicit block), bits >> 20 = bit index within the packed BOOL word
    ///   REC  = *(CTX+4) + 4 * *(CTX+48)  element record; its word 10 holds that flag
    ///
    /// Both candidate offsets are emitted for every entry, so the two record forms
    /// (compact/f32 and wide/f64) are covered without deciding here which applies.
    /// The table only ever describes the current element's type, so coverage comes
    /// from walking real elements and dumping once per newly met type.
    internal static class DabSlotExport
    {
        internal const int DefaultElementBudget = 300000;

        private const int RvaDbStack = 0x18E4024;
        private const int MaxEntries = 8192;
        private const int MaxEntryDwords = 64;
        private const int MinEntryDwords = 9;   // need index 5 (+20) and index 8 (+32)
        private const int MaxVariantsPerNoun = 2;
        private const uint MemCommit = 0x1000;
        private const uint PageNoAccess = 0x01;
        private const uint PageGuard = 0x100;

        [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
        private static extern IntPtr GetModuleHandle(string moduleName);

        [StructLayout(LayoutKind.Sequential)]
        private struct MemoryBasicInformation
        {
            public IntPtr BaseAddress;
            public IntPtr AllocationBase;
            public uint AllocationProtect;
            public IntPtr RegionSize;
            public uint State;
            public uint Protect;
            public uint Type;
        }

        [DllImport("kernel32.dll")]
        private static extern int VirtualQuery(IntPtr address, out MemoryBasicInformation buffer, int length);

        private sealed class TypeSlots
        {
            public int Hash;
            public string Noun = "";
            public int RecFlags;
            public int Count;
            public string DbType = "";
            public readonly List<int[]> Entries = new List<int[]>();
        }

        /// An AccessViolationException is not catchable on .NET 4, so every read is
        /// gated on VirtualQuery rather than tried and recovered from.
        private static bool Readable(int address, int bytes)
        {
            if (address <= 0x10000 || bytes <= 0) return false;
            MemoryBasicInformation mbi;
            int size = Marshal.SizeOf(typeof(MemoryBasicInformation));
            if (VirtualQuery(new IntPtr(address), out mbi, size) == 0) return false;
            if (mbi.State != MemCommit) return false;
            if ((mbi.Protect & PageNoAccess) != 0 || (mbi.Protect & PageGuard) != 0) return false;
            long end = mbi.BaseAddress.ToInt64() + mbi.RegionSize.ToInt64();
            return address + (long)bytes <= end;
        }

        private static bool TryRead(int address, out int value)
        {
            value = 0;
            if (!Readable(address, 4)) return false;
            value = Marshal.ReadInt32(new IntPtr(address));
            return true;
        }

        private static bool TryDumpCurrent(out TypeSlots slots)
        {
            slots = null;
            IntPtr core = GetModuleHandle("core.dll");
            if (core == IntPtr.Zero) return false;

            int stack;
            if (!TryRead(core.ToInt32() + RvaDbStack, out stack) || stack == 0) return false;
            int idx;
            if (!TryRead(stack + 8, out idx) || idx < 0 || idx > 4096) return false;
            int ctx = stack + 60 * idx + 16;
            int table;
            if (!TryRead(ctx, out table) || table == 0) return false;

            int hash;
            int count;
            if (!TryRead(table + 4, out hash) || hash == 0) return false;
            if (!TryRead(table + 36, out count) || count <= 0 || count > MaxEntries) return false;

            TypeSlots s = new TypeSlots();
            s.Hash = hash;
            s.Count = count;

            int recBase;
            int recIndex;
            if (TryRead(ctx + 4, out recBase) && TryRead(ctx + 48, out recIndex))
            {
                int flags;
                if (TryRead(recBase + 4 * recIndex + 40, out flags)) s.RecFlags = flags;
            }

            int cursor = 0;
            for (int i = 0; i < count; i++)
            {
                int entry = table + 4 * (cursor + 14);
                int step;
                if (!TryRead(entry + 4, out step) || step <= 0 || step > MaxEntryDwords) break;
                int take = step < MinEntryDwords ? MinEntryDwords : step;
                int[] raw = new int[take];
                bool ok = true;
                for (int k = 0; k < take; k++)
                {
                    if (!TryRead(entry + 4 * k, out raw[k])) { ok = false; break; }
                }
                if (!ok) break;
                s.Entries.Add(raw);
                cursor += step;
            }

            slots = s;
            return s.Entries.Count > 0;
        }

        private static long Signature(TypeSlots s)
        {
            unchecked
            {
                long h = 1469598103934665603L;
                h = (h ^ s.Hash) * 1099511628211L;
                for (int i = 0; i < s.Entries.Count; i++)
                {
                    int[] raw = s.Entries[i];
                    for (int k = 0; k < raw.Length; k++) h = (h ^ raw[k]) * 1099511628211L;
                }
                return h;
            }
        }

        internal static string Run(string path, int elementBudget)
        {
            MDB mdb = MDB.CurrentMDB;
            if (mdb == null) return "SLOTS ERROR: no current MDB";

            Db[] dbs;
            try { dbs = mdb.GetDBArray(); }
            catch (Exception ex) { return "SLOTS ERROR: GetDBArray " + ex.Message; }
            if (dbs == null) return "SLOTS ERROR: no DBs";

            Dictionary<string, TypeSlots> found = new Dictionary<string, TypeSlots>();
            Dictionary<string, int> variants = new Dictionary<string, int>();
            int visited = 0;
            int walked = 0;

            for (int i = 0; i < dbs.Length && visited < elementBudget; i++)
            {
                Db db = dbs[i];
                if (db == null) continue;
                DbElement world;
                try { world = db.World; }
                catch { continue; }
                if (world == null || !world.IsValid) continue;
                walked++;
                Walk(world, db, found, variants, ref visited, elementBudget);
            }

            Write(path, found, dbs.Length, walked, visited);
            return "SLOTS OK dbs=" + walked.ToString(CultureInfo.InvariantCulture)
                 + " elements=" + visited.ToString(CultureInfo.InvariantCulture)
                 + " nouns=" + variants.Count.ToString(CultureInfo.InvariantCulture)
                 + " tables=" + found.Count.ToString(CultureInfo.InvariantCulture)
                 + " -> " + path;
        }

        private static void Walk(
            DbElement root,
            Db db,
            Dictionary<string, TypeSlots> found,
            Dictionary<string, int> variants,
            ref int visited,
            int budget)
        {
            Stack<DbElement> pending = new Stack<DbElement>();
            pending.Push(root);
            while (pending.Count > 0 && visited < budget)
            {
                DbElement e = pending.Pop();
                visited++;
                Probe(e, db, found, variants);
                DbElement[] members = null;
                try { members = e.Members(); }
                catch { }
                if (members == null) continue;
                for (int i = 0; i < members.Length; i++)
                {
                    DbElement m = members[i];
                    if (m == null) continue;
                    bool valid;
                    try { valid = m.IsValid; }
                    catch { continue; }
                    if (valid) pending.Push(m);
                }
            }
        }

        private static void Probe(
            DbElement e,
            Db db,
            Dictionary<string, TypeSlots> found,
            Dictionary<string, int> variants)
        {
            string noun;
            try { noun = e.GetElementType().Name; }
            catch { return; }
            if (string.IsNullOrEmpty(noun)) return;

            int seen;
            if (variants.TryGetValue(noun, out seen) && seen >= MaxVariantsPerNoun) return;

            // Reading any attribute is what makes dabacon load this type's table into
            // the current context; without it the table would still be the last one.
            try { e.GetAsString(DbAttributeInstance.NAME); }
            catch { }

            TypeSlots s;
            if (!TryDumpCurrent(out s)) return;
            s.Noun = noun;
            try { s.DbType = db.Type.ToString(); }
            catch { }

            string key = s.Hash.ToString(CultureInfo.InvariantCulture)
                       + ":" + Signature(s).ToString("x", CultureInfo.InvariantCulture);
            if (found.ContainsKey(key)) return;
            found[key] = s;
            variants[noun] = seen + 1;
        }

        private static void Write(string path, Dictionary<string, TypeSlots> found, int dbs, int walked, int visited)
        {
            StringBuilder sb = new StringBuilder(1 << 22);
            sb.Append("{\n \"meta\":{");
            sb.Append("\"coreBase\":\"0x").Append(GetModuleHandle("core.dll").ToInt64().ToString("x", CultureInfo.InvariantCulture)).Append("\"");
            sb.Append(",\"rvaDbStack\":\"0x").Append(RvaDbStack.ToString("x", CultureInfo.InvariantCulture)).Append("\"");
            sb.Append(",\"dbsInMdb\":").Append(dbs.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"dbsWalked\":").Append(walked.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"elements\":").Append(visited.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"tables\":").Append(found.Count.ToString(CultureInfo.InvariantCulture));
            sb.Append("},\n \"tables\":[");

            bool first = true;
            foreach (KeyValuePair<string, TypeSlots> kv in found)
            {
                TypeSlots s = kv.Value;
                if (!first) sb.Append(",");
                first = false;
                sb.Append("\n  {\"noun\":\"").Append(s.Noun).Append("\"");
                sb.Append(",\"hash\":").Append(s.Hash.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"dbType\":\"").Append(s.DbType).Append("\"");
                sb.Append(",\"recFlags\":").Append(s.RecFlags.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"f32Form\":").Append((s.RecFlags & 0x20000000) == 0 ? "true" : "false");
                sb.Append(",\"declared\":").Append(s.Count.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"read\":").Append(s.Entries.Count.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"entries\":[");
                for (int i = 0; i < s.Entries.Count; i++)
                {
                    int[] raw = s.Entries[i];
                    if (i > 0) sb.Append(",");
                    sb.Append("\n   {\"k\":").Append(raw[0].ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"n\":").Append(raw[1].ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"o20\":").Append(raw[5].ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"o32\":").Append(raw[8].ToString(CultureInfo.InvariantCulture));
                    sb.Append(",\"raw\":[");
                    for (int k = 0; k < raw.Length; k++)
                    {
                        if (k > 0) sb.Append(",");
                        sb.Append(raw[k].ToString(CultureInfo.InvariantCulture));
                    }
                    sb.Append("]}");
                }
                sb.Append("\n  ]}");
            }
            sb.Append("\n ]\n}\n");

            string dir = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir)) Directory.CreateDirectory(dir);
            File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
        }
    }

    /// Short-named twin of NounLayoutExport. PML object names come straight from
    /// the class name, and a 16-character one is right at the edge of what PML
    /// accepts, so this keeps a known-safe name available.
    [PMLNetCallable()]
    public class GmSlots
    {
        [PMLNetCallable()]
        public GmSlots()
        {
        }

        [PMLNetCallable()]
        public void Assign(GmSlots other)
        {
        }

        [PMLNetCallable()]
        public string Run(string path, string budget)
        {
            try
            {
                int n;
                if (!int.TryParse(budget, NumberStyles.Integer, CultureInfo.InvariantCulture, out n) || n <= 0)
                    n = DabSlotExport.DefaultElementBudget;
                return DabSlotExport.Run(path, n);
            }
            catch (Exception ex)
            {
                return "ERROR " + ex.GetType().Name + ": " + ex.Message;
            }
        }
    }

#if !PMLONLY
    public class NounLayoutAddin : IAddin
    {
        private const string DefaultOutput =
            @"D:\work\plant-code\old\gen-model\output\noun_layout.json";

        public string Name
        {
            get { return "GenModel Noun Layout Export"; }
        }

        public string Description
        {
            get { return "One-shot dump of every element type's ordered attribute layout."; }
        }

        public void Start(ServiceManager serviceManager)
        {
            Journal("addin-start");
            // The dictionary is not queryable yet at addin-start time; the first idle
            // is the earliest point the module is fully up, and it is the UI thread.
            Application.Idle += OnIdle;
        }

        public void Stop()
        {
        }

        private void OnIdle(object sender, EventArgs e)
        {
            Application.Idle -= OnIdle;
            Journal("idle-reached");
            string path = Environment.GetEnvironmentVariable("GENMODEL_NOUN_LAYOUT_OUT");
            if (string.IsNullOrEmpty(path)) path = DefaultOutput;
            try
            {
                Journal(new NounLayoutExport().Export(path));
            }
            catch (Exception ex)
            {
                Journal("THREW " + ex.GetType().Name + ": " + ex.Message);
            }

            try
            {
                string slots = Environment.GetEnvironmentVariable("GENMODEL_NOUN_SLOTS_OUT");
                if (string.IsNullOrEmpty(slots))
                    slots = Path.Combine(Path.GetDirectoryName(path) ?? ".", "noun_descriptor_slots.json");
                Journal(DabSlotExport.Run(slots, ElementBudget()));
            }
            catch (Exception ex)
            {
                Journal("SLOTS THREW " + ex.GetType().Name + ": " + ex.Message);
            }
        }

        private static int ElementBudget()
        {
            int budget;
            string raw = Environment.GetEnvironmentVariable("GENMODEL_NOUN_SLOTS_BUDGET");
            if (!string.IsNullOrEmpty(raw)
                && int.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out budget)
                && budget > 0)
            {
                return budget;
            }
            return DabSlotExport.DefaultElementBudget;
        }

        private static void Journal(string line)
        {
            try
            {
                string log = Environment.GetEnvironmentVariable("GENMODEL_NOUN_LAYOUT_LOG");
                if (string.IsNullOrEmpty(log))
                    log = Path.ChangeExtension(DefaultOutput, null) + "_export.log";
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
#endif
}
