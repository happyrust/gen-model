"""Run a named, archived E3D command probe against the isolated AMS copy."""
import sys
import equi8000_live as e

folder=e.OUT/sys.argv[1]
folder.mkdir(exist_ok=False)
e.tty(folder,sys.argv[2:])
print(folder)
