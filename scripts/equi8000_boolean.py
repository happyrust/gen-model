"""Real AMS equipment subtraction fixture; all edits use the isolated project."""
import sys, json, subprocess
import equi8000_live as e

BASE='/CODEX_ROCKS8000_BOOL_BASE'
CUT='/CODEX_ROCKS8000_BOOL_CUT'
phase=sys.argv[1]
folder=e.OUT/('boolean-'+phase); folder.mkdir(exist_ok=False)
commands={
 'solid':['='+e.ROOT,'NEW BOX '+BASE,'XLEN 1000','YLEN 1000','ZLEN 400','POS E 1500 N 0 U 0'],
 'hole':[BASE,'NEW NBOX '+CUT,'XLEN 300','YLEN 300','ZLEN 600'],
 'widen':[CUT,'XLEN 500'],
 'move':[CUT,'POS E 150 N 0 U 0'],
 'fill':[CUT,'DELETE NBOX'],
 'restore':[BASE,'DELETE BOX'],
}
e.tty(folder,commands[phase]+['Q CE','Q REF',"SAVEWORK 'RocksDB boolean "+phase+"'"])
print(folder)
