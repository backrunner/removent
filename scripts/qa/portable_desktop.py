#!/usr/bin/env python3
"""Independent GUI event oracle; run under an isolated Xvfb/Windows desktop."""
import argparse, json, os, subprocess, tempfile, threading, tkinter as tk
from pathlib import Path
from PIL import ImageGrab

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--bin-dir",type=Path,default=Path("target/release"))
    parser.add_argument("--probe",default="desktop_smoke")
    parser.add_argument("--evidence-dir",type=Path,default=Path("target/qa-evidence"))
    args=parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    if os.name == "nt":
        import ctypes
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    binary=args.bin_dir.resolve()/"examples"/(args.probe + (".exe" if os.name=="nt" else ""))
    root=tk.Tk(); root.geometry("640x480+0+0"); root.title("Removent isolated desktop test")
    root.configure(background="#ff0000"); root.attributes("-topmost",True)
    entry=tk.Entry(root,font=("Arial",20)); entry.place(x=50,y=170,width=500,height=60)
    entry.insert(0,"replace me")
    def select_all(event):
        entry.selection_range(0,tk.END)
        return "break"
    entry.bind("<Control-a>", select_all)
    clicks=[]; entry.bind("<Button-1>",lambda event:clicks.append((event.x,event.y)))
    outcome=[]
    def worker():
        with tempfile.TemporaryDirectory(prefix="removent-desktop-") as data:
            env=dict(os.environ,REMOVENT_DESKTOP_TEST="1",REMOVENT_DATA_DIR=data,
                     REMOVENT_TEST_X=str(target[0]),REMOVENT_TEST_Y=str(target[1]))
            try:
                result=subprocess.run([str(binary)],env=env,capture_output=True,text=True,timeout=45)
                outcome.append(result)
            except Exception as e: outcome.append(e)
    def begin():
        root.lift(); entry.focus_force()
        target[:] = [entry.winfo_rootx()+entry.winfo_width()//2, entry.winfo_rooty()+entry.winfo_height()//2]
        threading.Thread(target=worker,daemon=True).start(); root.after(100,poll)
    def poll():
        if not outcome: root.after(100,poll); return
        root.after(500,finish)
    def finish():
        result=outcome[0]
        try:
            ImageGrab.grab().save(args.evidence_dir/(args.probe+".png"))
            (args.evidence_dir/(args.probe+".json")).write_text(json.dumps({"target":target,"clicks":clicks,"text":entry.get()},ensure_ascii=True))
            if isinstance(result,Exception): raise result
            print(result.stdout,flush=True); print(result.stderr,flush=True)
            assert result.returncode==0, f"Native probe exited {result.returncode}"
            assert clicks, "No native click reached the independent UI"
            assert entry.get()=="hello中文", f"Native typing mismatch: {entry.get()!r}"
            print("PASS: independent UI received native click, Ctrl+A and exact text",flush=True)
        except BaseException as e: outcome.append(e)
        finally: root.destroy()
    target=[]
    root.after(1500,begin); root.mainloop()
    if len(outcome)>1: raise outcome[-1]
if __name__=="__main__": main()
