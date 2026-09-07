"""Prepare paired movement macros for an already COPIED test branch.

Does not launch E3D. Refuses the screenshot's production root. Dynamic baseline
POS/ORI/member identities must be captured before execution; restore is verified
against that baseline, never against rounded historical coordinates.
"""
import argparse
from pathlib import Path
import re


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--scratch-root', required=True)
    p.add_argument('--out', required=True)
    a = p.parse_args()
    if not re.fullmatch(r'\d+/\d+', a.scratch_root) or a.scratch_root == '24384/26229':
        p.error('supply the actual refno of the copied disposable branch')
    out = Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    movements = {'M01':('ELBO 3','U','D',500), 'M02':('ELBO 3','E','W',500),
                 'M03':('VALV 1','U','D',200), 'M04':(None,'U','D',1000)}
    for case, (selector, direction, inverse, distance) in movements.items():
        for leg, axis in [('apply',direction),('restore',inverse)]:
            lines = [f'ALPHA LOG "{out.as_posix()}/{case}-{leg}.log" OVER',
                     f'$P {case}-{leg}-ALIVE', '='+a.scratch_root, 'Q REF', 'Q MEMB']
            if selector: lines.append(selector)
            lines += ['Q REF','Q POS','Q ORI',f'BY {axis} {distance}mm','Q POS',
                      f"SAVEWORK 'pipe26229 {case} {leg}'",f'$P {case}-{leg}-DONE','ALPHA LOG END']
            (out/f'{case}-{leg}.mac').write_text('\n'.join(lines)+'\n', encoding='utf-8')
    print(f'Prepared 4 apply/restore pairs in {out}; no E3D mutation executed.')


if __name__ == '__main__': main()
