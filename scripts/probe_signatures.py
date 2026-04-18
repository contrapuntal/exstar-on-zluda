"""
Capture prologue signatures at each known-good RVA in v1.1.0.16, then scan
v1.1.1-8 for the same bytes to find the new RVA.

Output: for each probe, prints old_rva, sig, new_rva (or multiple/none), and
emits a Rust-ready probe entry.
"""
import struct
import sys
from pathlib import Path

BASELINES = Path(r"%USERPROFILE%\proj\exstar-baselines")
OLD = BASELINES / "v1.1.0.16"
NEW = BASELINES / "v1.1.1-8"

SIG_LEN = 16  # bytes to capture at each probe's RVA

# (module_file_name, label, rva)
PROBES = [
    # --- EXStar Hub.exe ---
    ("EXStar Hub.exe", "entry_6940",            0x6940),
    ("EXStar Hub.exe", "lambda_6dc0",           0x6DC0),
    ("EXStar Hub.exe", "signal_slot_bc30",      0xBC30),
    ("EXStar Hub.exe", "entry_d070",            0xD070),
    ("EXStar Hub.exe", "entry_f0f8",            0xF0F8),
    ("EXStar Hub.exe", "wrapper_f9ec",          0xF9EC),
    ("EXStar Hub.exe", "wrapper_fa3c",          0xFA3C),
    ("EXStar Hub.exe", "wrapper_fac4",          0xFAC4),
    ("EXStar Hub.exe", "init_gate_f82c",        0xF82C),
    ("EXStar Hub.exe", "init_check_f6c0",       0xF6C0),
    ("EXStar Hub.exe", "post_d070_check_10390", 0x10390),
    ("EXStar Hub.exe", "guard_a6e0",            0xA6E0),
    # --- Sn3DprocessManager.exe ---
    ("Sn3DprocessManager.exe", "kill_all_e1a0",    0xE1A0),
    ("Sn3DprocessManager.exe", "load_config_ef30", 0xEF30),
    ("Sn3DprocessManager.exe", "kill_one_e560",    0xE560),
    ("Sn3DprocessManager.exe", "env_detect_5e30",  0x5E30),
    ("Sn3DprocessManager.exe", "launch_f5f0",      0xF5F0),
    # --- scanservice.exe ---
    ("scanservice.exe", "entry_6a40", 0x6A40),
    # --- AppUi.dll ---
    ("AppUi.dll", "handleShowPassport",    0x4DCF6),
    ("AppUi.dll", "startPassportProcess",  0x6E0DE),
    ("AppUi.dll", "handlePassportData",    0x6E8A0),
    # --- Sn3DUserPassport.dll ---
    ("Sn3DUserPassport.dll", "handleShowPassportCmd", 0x3BFC1),
    ("Sn3DUserPassport.dll", "handleLoginSuccess",    0x39ED0),
    # --- Sn3DProcessPlugin.dll ---
    ("Sn3DProcessPlugin.dll", "connectImpl_wrapper_6d60",  0x6D60),
    ("Sn3DProcessPlugin.dll", "plugin_connectToHub_72c0",  0x72C0),
    ("Sn3DProcessPlugin.dll", "connectAndRegister_4c50",   0x4C50),
]


class PE:
    """Minimal PE parser — enough to translate RVA <-> file offset and scan .text."""
    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        if self.data[:2] != b"MZ":
            raise ValueError(f"{path}: not MZ")
        pe_off = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe_off:pe_off + 4] != b"PE\0\0":
            raise ValueError(f"{path}: PE signature missing at {pe_off:#x}")
        coff = pe_off + 4
        (machine, num_sections, _, _, _, sz_opt_hdr, _) = struct.unpack_from(
            "<HHIIIHH", self.data, coff
        )
        opt = coff + 20
        magic = struct.unpack_from("<H", self.data, opt)[0]
        if magic == 0x10B:
            self.arch = "x86"
            sec_hdr = opt + 224  # PE32
        elif magic == 0x20B:
            self.arch = "x64"
            sec_hdr = opt + 240  # PE32+
        else:
            raise ValueError(f"{path}: unknown opt magic {magic:#x}")
        sec_hdr = opt + sz_opt_hdr  # safer: use reported optional header size
        self.sections = []
        for i in range(num_sections):
            base = sec_hdr + i * 40
            name = self.data[base:base + 8].rstrip(b"\0").decode("ascii", "replace")
            (virt_sz, virt_addr, raw_sz, raw_ptr, _, _, _, _, chars) = struct.unpack_from(
                "<IIIIIIHHI", self.data, base + 8
            )
            self.sections.append({
                "name": name, "va": virt_addr, "vsz": virt_sz,
                "raw": raw_ptr, "raw_sz": raw_sz, "chars": chars,
            })

    def rva_to_file_off(self, rva: int) -> int | None:
        for s in self.sections:
            if s["va"] <= rva < s["va"] + max(s["vsz"], s["raw_sz"]):
                return s["raw"] + (rva - s["va"])
        return None

    def read_at_rva(self, rva: int, length: int) -> bytes | None:
        off = self.rva_to_file_off(rva)
        if off is None or off + length > len(self.data):
            return None
        return self.data[off:off + length]

    def text_section(self):
        for s in self.sections:
            if s["name"] == ".text":
                return s
        return None

    def scan_for(self, needle: bytes) -> list[int]:
        """Return RVAs in .text where `needle` occurs."""
        sec = self.text_section()
        if sec is None:
            return []
        start, end = sec["raw"], sec["raw"] + sec["raw_sz"]
        blob = self.data[start:end]
        hits = []
        i = 0
        while True:
            j = blob.find(needle, i)
            if j < 0:
                break
            hits.append(sec["va"] + j)
            i = j + 1
        return hits


def fmt_bytes(b: bytes) -> str:
    return " ".join(f"{x:02x}" for x in b)


def main():
    modules = {}
    for fname in sorted({m for m, _, _ in PROBES}):
        modules[fname] = (PE(OLD / fname), PE(NEW / fname))

    print(f"{'module':<28} {'label':<26} {'old_rva':>9}  {'signature':<48}  result")
    print("-" * 130)

    rust_entries = []
    for fname, label, old_rva in PROBES:
        old_pe, new_pe = modules[fname]
        sig = old_pe.read_at_rva(old_rva, SIG_LEN)
        if sig is None:
            print(f"{fname:<28} {label:<26} {old_rva:>#9x}  <unreadable in old binary>")
            continue

        # 1. Same RVA in new binary?
        same_rva = new_pe.read_at_rva(old_rva, SIG_LEN)
        same_match = same_rva == sig

        # 2. Scan new binary for signature
        hits = new_pe.scan_for(sig)

        if same_match:
            result = f"UNCHANGED new_rva={old_rva:#x}"
            new_rva = old_rva
        elif len(hits) == 1:
            new_rva = hits[0]
            result = f"MOVED new_rva={new_rva:#x}"
        elif len(hits) == 0:
            new_rva = None
            result = "NOT FOUND in new .text"
        else:
            new_rva = None
            hit_list = ", ".join(f"{h:#x}" for h in hits[:5])
            result = f"AMBIGUOUS {len(hits)} matches: {hit_list}"

        print(f"{fname:<28} {label:<26} {old_rva:>#9x}  {fmt_bytes(sig):<48}  {result}")

        rust_entries.append({
            "module": fname, "label": label,
            "old_rva": old_rva, "new_rva": new_rva,
            "sig": sig, "same_match": same_match,
        })

    # Also emit the same-rva/new-bytes snapshot for probes that moved or vanished,
    # so we can manually inspect whether v1.1.1-8 broke the prologue at the old spot.
    print("\n--- Bytes at old RVA in v1.1.1-8 (for vanished / moved probes) ---")
    for e in rust_entries:
        if not e["same_match"]:
            fname = e["module"]
            new_pe = modules[fname][1]
            new_bytes = new_pe.read_at_rva(e["old_rva"], SIG_LEN)
            print(f"  {fname:<28} {e['label']:<26} old_rva={e['old_rva']:#x}")
            print(f"    old_sig : {fmt_bytes(e['sig'])}")
            print(f"    new@old : {fmt_bytes(new_bytes) if new_bytes else '<unreadable>'}")

    # Rust candidate arrays: for each probe, list (rva, sig) candidates.
    # Primary = new_rva (if known) for v1.1.1-8; fallback = old_rva for v1.1.0.16.
    print("\n--- Rust probe candidates (copy into lib.rs) ---")
    for e in rust_entries:
        sig_arr = ", ".join(f"0x{b:02x}" for b in e["sig"])
        if e["new_rva"] is None:
            # No match in v1.1.1-8: only old_rva, will fail gracefully on new version
            print(
                f"// {e['module']} :: {e['label']} — NO v1.1.1-8 match, v1.1.0.16 only"
            )
            print(
                f'probe("{e["label"]}", &[(0x{e["old_rva"]:x}, [{sig_arr}])]),'
            )
        elif e["new_rva"] == e["old_rva"]:
            print(
                f"// {e['module']} :: {e['label']} — unchanged RVA, single candidate"
            )
            print(
                f'probe("{e["label"]}", &[(0x{e["old_rva"]:x}, [{sig_arr}])]),'
            )
        else:
            print(
                f"// {e['module']} :: {e['label']} — moved 0x{e['old_rva']:x} -> 0x{e['new_rva']:x}"
            )
            print(
                f'probe("{e["label"]}", &['
                f"(0x{e['new_rva']:x}, [{sig_arr}]), "
                f"(0x{e['old_rva']:x}, [{sig_arr}])"
                f"]),"
            )


if __name__ == "__main__":
    main()
