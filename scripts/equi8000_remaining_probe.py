"""Read-only E3D discovery of remaining primitive families and their real owners."""
import equi8000_live as e
folder=e.OUT/'remaining-source-hierarchy';folder.mkdir(exist_ok=False)
nouns='ABOX ACYLI APYRA ARTOR ACTOR ASNOU ACONE ADISH ASLCY AEXTR APOLYH AREVO BOXI GMSE LCYL LINE LPYR LSNO SBOX SCON SCTO SCYL SDIS SDSH SEXT SLINE SLOO SREV SRTO SSLC SSPH SVER TUBE GMSS SANN SPRO SPVE SREC NBXI NGMS NLCY NLPY NLSN NSBO NSCO NSCT NSCY NSDS NSEX NSRE NSRT NSSL NSSP NTUB'.split()
commands=[]
for noun in nouns:
    commands += [f'$P REMAINING-{noun}',f'VAR !items COLLECT ALL {noun}',
        'DO !item VALUES !items',' $!item',' Q REF',' Q TYPE',' Q OWNER',' Q MEMB',
        ' OWNER',' Q REF',' Q TYPE',' Q OWNER',' BREAK','ENDDO']
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig',errors='replace'))
