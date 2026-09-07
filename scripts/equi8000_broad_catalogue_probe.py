"""Read every program in the boolean-heavy AMS8000 valve GMSET."""
import equi8000_live as e


folder = e.OUT / 'remaining-broad-native-programs'
folder.mkdir(exist_ok=False)
commands = []
for number in range(548243, 548279):
    ref = f'13245/{number}'
    commands += [f'$P PROGRAM-{ref}', f'={ref}', 'Q REF', 'Q TYPE', 'Q ATT']
e.tty(folder, commands)
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
