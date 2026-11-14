"""Independent native window that makes a live remote desktop visibly change."""
import tkinter as tk

root = tk.Tk()
root.title("Tray video acceptance stimulus")
root.overrideredirect(True)
root.attributes("-topmost", True)
root.geometry(f"200x80+20+{root.winfo_screenheight() - 100}")
label = tk.Label(root, text="", font=("Arial", 18))
label.pack(fill="both", expand=True)


def animate(frame=0):
    label.configure(text=f"Video {frame}", bg="#20a060" if frame % 2 else "#a04060", fg="white")
    root.after(100, animate, frame + 1)


animate()
root.mainloop()
