# DB_UserChanges 事件级证据：布局校准与采集器就绪报告

本文记录 2026-10-29 用 ida-bridge（`D:\ida_scratch\ida-bridge-core-readonly\core.dll.i64`，
idalib 直连）对 `DB_UserChanges` 做的布局校准。**结论先行**：采集器要用的 refno 偏移与
七个符号地址全部核实，`db_userchanges.c` 的反编译文本与二进制一致；上一版文档里
「core.dll 导出表在磁盘上被混淆」的说法是**我解析脚本的字段读错**，已在 §3 更正。

## 1. 两个库是同一份二进制

| 项 | shadow `core.dll`（TTY 实际加载的） | `ida-bridge-core-readonly\core.dll.i64` |
|---|---|---|
| 文件大小 | 50 071 544 | 402 MB（含分析） |
| PE TimeDateStamp | `0x67CA7EDC` | — |
| PE CheckSum | `0x02FC51E9` | — |
| PE 头 ImageBase | `0x10000000` | IDA 库 `0x05170000` |
| 段表 | `.text/.rdata/.data/_RDATA/.rsrc/.reloc/.vlizer` | 同 |

ImageBase 不同只是 IDA 打开时的加载基址与 PE 头不一致（`.vlizer` 是 VMProtect 段，
这类加壳二进制常见）。**判定同一份的依据是机器码**：把 IDA 里的地址减去 `0x5170000`
得到 RVA，再按 RVA 去 shadow 文件里取字节，`elementCreated`（RVA `0x00817A90`）处是

```
55 8b ec 6a ff 68 48 40 b7 10 64 a1 00 00 00 00 50 83 ec 24
a1 f0 d8 e5 10 33 c5 89 45 f0 56 57 50 8d 45 f4 64 a3 ...
```

即 `push ebp / mov ebp,esp / push 0FFFFFFFFh / push seh / mov eax,fs:0 / push eax /
sub esp,24h / mov eax,___security_cookie / xor eax,ebp / mov [ebp-10h],eax /
push esi / push edi / push eax / lea eax,[ebp-0Ch] / mov fs:0,eax / mov esi,ecx` ——
标准 MSVC `/GS` prologue，末尾 `mov esi, ecx` 坐实 `__thiscall`。`elementDeleted`、
`attributeModified`、`DB_Attribute::name` 四处同样对上。

## 2. `DB_Element` 的 refno 在 `+0x0C` / `+0x10`（已从汇编坐实）

`elementCreated` 尾部：

```
0x05987b26  push    edi
0x05987b27  lea     ecx, [esi+30h]
0x05987b2a  call    ??4DB_Ref@@QAEAAV0@ABV0@@Z      ; DB_Ref::operator=
0x05987b2f  mov     eax, [edi+0Ch]
0x05987b32  mov     [esi+3Ch], eax
0x05987b35  mov     eax, [edi+10h]
0x05987b38  mov     [esi+40h], eax
```

`edi` = `a2`（`DB_Element const&`），`esi` = `this`（`DB_UserChanges*`）。两条 `mov`
把 `DB_Element+0x0C` / `+0x10` 搬进 `DB_UserChanges+0x3C` / `+0x40`，正是 PDMS refno 的
`(word0, word1)` 两个 32 位字。`attributeModified(qual)` 里也有同样的搬移
（`0x0598712f mov eax,[ebx+0Ch]` / `0x05987132 mov [esi+0Ch],eax`）。

采集器的 `readRef()` 读 `+0x0C` / `+0x10` **正确**。

## 3. 更正：导出表没有混淆，是我读错了字段

`IMAGE_EXPORT_DIRECTORY` 的真实布局是

```
+0  Characteristics   +4  TimeDateStamp   +8  MajorVersion(2)  +10 MinorVersion(2)
+12 Name              +16 Base            +20 NumberOfFunctions
+24 NumberOfNames     +28 AddressOfFunctions
+32 AddressOfNames    +36 AddressOfNameOrdinals
```

上一版脚本把 `+28` 的第一个 `DWORD` 当成 `AddressOfNames`（实际是 `AddressOfFunctions`），
又用「名字表下标」去查 `AddressOfFunctions`（应当先用 `AddressOfNameOrdinals[i]` 换成
序号），所以读出来是代码字节。修正后 shadow 的导出表**完全正常**：

```
ImageBase 0x10000000  NumberOfFunctions 19625  NumberOfNames 19625
AddressOfFunctions RVA 0x00ca4028 / AddressOfNames RVA 0x00cb72cc / Ordinals RVA 0x00cca570
  RVA 0x00762690  ord 15994  ?name@DB_Attribute@@QBEABV?$basic_string@DU...
  RVA 0x00817010  ord 12796  ?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@@Z
  RVA 0x00817090  ord 12797  ?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@ABVDB_Qualifier@@@Z
  RVA 0x00817520  ord 13060  ?combine@DB_UserChanges@@QAEXABV1@@Z
  RVA 0x00817a00  ord 13296  ?currentInstance@DB_UserChanges@@SAAAV1@XZ
  RVA 0x00817a90  ord 13714  ?elementCreated@DB_UserChanges@@QAEXABVDB_Element@@@Z
  RVA 0x00817b70  ord 13715  ?elementDeleted@DB_UserChanges@@QAEXABVDB_Element@@@Z
  RVA 0x00817ea0  ord 13716  ?elementIncluded@DB_UserChanges@@QAEXABVDB_Element@@0@Z
  RVA 0x00818040  ord 13718  ?elementReordered@DB_UserChanges@@QAEXABVDB_Element@@@Z
```

八个 RVA 与 IDA 库地址（减 `0x5170000`）**逐个精确相等**。因此采集器用
`Module.getExportByName(mangled)` 拿地址是可靠的；Frida 的 `getExportByName` 走的是
loader 解析，同样不受影响。

## 4. `ATT_MEMB` 走依赖级联，不是普通属性 map（P82/P83 的分流点）

`attributeModified(qual)` 在 `0x05987176` 处：

```
0x05987176  cmp     eax, ?ATT_MEMB@@3QBVDB_Attribute@@B
0x0598717c  jnz     short loc_5987195        ; 不是 ATT_MEMB -> 走普通属性路径
0x05987181  lea     eax, [ebp+var_C8]
0x05987188  lea     ecx, [edi+18h]
0x0598718b  call    sub_5986450              ; 往 this+0x18 那个桶插
0x05987190  jmp     loc_59873CF
```

`loc_59873CF` 之后是

```
0x059873f0  call    ?instance@DB_UserChangesDependency@@SAPAV1@XZ
0x059873f7  call    ?getDependencies@DB_UserChangesDependency@@QBEXABVDB_Element@@PBVDB_Attribute@@...
```

即 `ATT_MEMB` 的记录走**依赖表**（`DB_UserChangesDependency`），与普通属性走
`this+0x28` 的 qualifier map 是两条不同的路。这直接支撑 P82/P83 的预期：
成员表变化记成 `attributeModified(elem, ATT_MEMB)`，**不是** `elementReordered`。

`elementReordered` 只在 `elementsChangedBetween` 的成员逐条比较里、码值 `== 3` 时调用
（`db_userchanges.c` 第 1124 行 `if ( v28 == 3 )`），这是另一条独立入口。

## 5. qualifier 分支：`stringQualifiers().size()` 是判定点

```
0x05987294  call    ?stringQualifiers@DB_Qualifier@@QBEABV?$vector@...
0x0598729d  call    sub_58286F0              ; 取 vector 首尾，算 size
0x059872c8  cmp     eax, 1
0x059872ce  jnz     short loc_5987316        ; size != 1 -> 另一条路
```

`size == 1` 与 `size != 1` 两条路分别往 `this+0x28`（qualifier map）插不同形状的键。
P84（qualifier 1→0）与 P85（同键两次写）要判的就是**走的是哪条、键长什么样**，
这正是运行时 `attributeModified` 两个重载 + qualifier 内容才能给出的信息。

## 6. 采集器状态

`scripts/e3d/increment_suite/tools/dc_events_probe.py` 已按上述校准修正：

- `readRef()` 读 `DB_Element+0x0C`/`+0x10`（§2 坐实）；
- `onEnter(args)` 的 `args[0]` 是第一个**栈**参数（`__thiscall` 的 `this` 在 `ecx`，
  不在 `args[0]`）——上一版写成 `args[1]` 会整体偏移一个参数，已修；
- 属性名通过 `DB_Attribute::name` 的 const getter 读（`std::string` 24 字节布局：
  `+0x10 _Mysize` / `+0x14 _Myres`，`_Myres >= 16` 时 `+0x00` 是堆指针）；
- 用 `Module.getExportByName(mangled)` 取地址（§3 确认导出表可用）。

JS 已通过 `frida.attach(self)` + `create_script` + `load()` 编译验证（`LOAD OK`）。

**仍未实跑**：AMS 库被 `aios-database.exe`（pid 70192）独占，`ams8000_0001` 与
`ams7998_0001` 都拿不到写句柄，起不了 TTY。实跑脚本与判据见
[`p79-p85-event-evidence.md`](./p79-p85-event-evidence.md) §2。
