"""The DLLs a Windows program needs from its own folder (its import table, followed through).

Used by release.ps1 to bundle only what tesseract.exe loads. Usage:
    python dll_closure.py <folder> <program.exe>
Prints one DLL name per line (as named in the folder).
"""
import os
import struct
import sys


def imports(path):
    with open(path, "rb") as f:
        b = f.read()
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    nsec = struct.unpack_from("<H", b, pe + 6)[0]
    opt_size = struct.unpack_from("<H", b, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", b, opt)[0]
    dd = opt + (112 if magic == 0x20B else 96)
    imp_rva = struct.unpack_from("<I", b, dd + 8)[0]
    delay_rva = struct.unpack_from("<I", b, dd + 13 * 8)[0]
    secs = []
    s = opt + opt_size
    for i in range(nsec):
        vsize, va, rsize, raw = struct.unpack_from("<IIII", b, s + i * 40 + 8)
        secs.append((va, max(vsize, rsize), raw))

    def off(rva):
        for va, size, raw in secs:
            if va <= rva < va + size:
                return rva - va + raw
        return None

    def name_at(rva):
        o = off(rva)
        return b[o:b.index(b"\0", o)].decode("ascii", "replace") if o is not None else None

    out = []
    for start, stride, name_field in ((imp_rva, 20, 12), (delay_rva, 32, 4)):
        o = off(start) if start else None
        while o is not None:
            n = struct.unpack_from("<I", b, o + name_field)[0]
            if n == 0:
                break
            nm = name_at(n)
            if nm:
                out.append(nm)
            o += stride
    return out


def closure(folder, exe):
    have = {f.lower(): f for f in os.listdir(folder) if f.lower().endswith(".dll")}
    need, todo = set(), [exe]
    while todo:
        for d in imports(os.path.join(folder, todo.pop())):
            k = d.lower()
            if k in have and k not in need:
                need.add(k)
                todo.append(have[k])
    return sorted(have[k] for k in need)


if __name__ == "__main__":
    for d in closure(sys.argv[1], sys.argv[2]):
        print(d)
