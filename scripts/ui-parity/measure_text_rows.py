import sys
from PIL import Image
from collections import Counter
o=Image.open(sys.argv[1]).convert('RGB')
W,H=o.size
o=o.crop((8,31,W-8,H-8)); W,H=o.size; px=o.load()
print('client',W,H)
def ismenu(c): r,g,b=c; return r>180 and g>120 and b<100 and r-b>110
rows=[y for y in range(H//2,H) if sum(1 for x in range(0,W//2) if ismenu(px[x,y]))>1]
groups=[]
for y in rows:
    if groups and y-groups[-1][1]<=3: groups[-1][1]=y
    else: groups.append([y,y])
for g0,g1 in groups:
    pts=[(x,y) for y in range(g0,g1+1) for x in range(0,W//2) if ismenu(px[x,y])]
    xs=[p[0] for p in pts]
    print('y',g0,g1,'h',g1-g0+1,'x',min(xs),max(xs),'bottom-gap',H-g1, Counter(px[p] for p in pts).most_common(1))
