# Usage: python -I selftest.py "<game folder>" "<path to libbackrooms_practice_tool.dll>"
# Self-test: boot the game's il2cpp runtime standalone, load the tool DLL, and check that every
# method/field it needs resolves and the hooks install (expects "pret" and no "erreur" in the log).
import ctypes as C, os, sys, time
game, dll = sys.argv[1], sys.argv[2]
log = os.path.splitext(dll)[0] + '.log'
if os.path.exists(log): os.remove(log)
os.add_dll_directory(game)
ga = C.CDLL(os.path.join(game, 'GameAssembly.dll'))
ga.il2cpp_set_data_dir(os.path.join(game, 'Inside the Backrooms_Data', 'il2cpp_data').encode())
ga.il2cpp_init(b'IL2CPP Root Domain')
C.WinDLL(dll)
for _ in range(40):
    time.sleep(0.5)
    text = open(log, encoding='utf-8').read() if os.path.exists(log) else ''
    if 'pret' in text or 'erreur' in text or 'abandon' in text: break
print(text, flush=True)
assert 'pret' in text and 'erreur' not in text, 'self-test failed'
print('SELFTEST OK', flush=True)
os._exit(0)
