#!/usr/bin/env python3
"""Independent GUI event oracle; run under an isolated Xvfb/Windows desktop."""
import argparse, os, subprocess, tempfile, threading, tkinter as tk
from pathlib import Path

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--bin-dir",type=Path,default=Path("target/release"))
    args=parser.parse_args()
    binary=args.bin_dir.resolve()/"examples"/("desktop_smoke.exe" if os.name=="nt" else "desktop_smoke")
    root=tk.Tk(); root.geometry("640x480+0+0"); root.title("Removent isolated desktop test")
    root.configure(background="#ff0000"); root.attributes("-topmost",True)
    entry=tk.Entry(root,font=("Arial",20)); entry.place(x=50,y=170,width=500,height=60)
    clicks=[]; entry.bind("<Button-1>",lambda event:clicks.append((event.x,event.y)))
    outcome=[]
    def worker():
        with tempfile.TemporaryDirectory(prefix="removent-desktop-") as data:
            env=dict(os.environ,REMOVENT_DESKTOP_TEST="1",REMOVENT_DATA_DIR=data)
            try:
                result=subprocess.run([str(binary)],env=env,capture_output=True,text=True,timeout=45)
                outcome.append(result)
            except Exception as e: outcome.append(e)
    def begin():
        root.lift(); entry.focus_force(); threading.Thread(target=worker,daemon=True).start(); root.after(100,poll)
    def poll():
        if not outcome: root.after(100,poll); return
        root.after(500,finish)
    def finish():
        result=outcome[0]
        try:
            if isinstance(result,Exception): raise result
            print(result.stdout,flush=True); print(result.stderr,flush=True)
            assert result.returncode==0, f"Native probe exited {result.returncode}"
            assert clicks, "No native click reached the independent UI"
            assert entry.get()=="hello", f"Native typing mismatch: {entry.get()!r}"
            print("PASS: real capture, decode, clipboard, native mouse and keyboard",flush=True)
        except BaseException as e: outcome.append(e)
        finally: root.destroy()
    root.after(1500,begin); root.mainloop()
    if len(outcome)>1: raise outcome[-1]
if __name__=="__main__": main()
