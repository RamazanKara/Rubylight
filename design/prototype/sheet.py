# Contact sheet for reviewing many shots in one image: python sheet.py out.png name1 name2 ...
import sys
from PIL import Image
out, names = sys.argv[1], sys.argv[2:]
H = int(next((a[2:] for a in names if a.startswith('h=')), 915))
names = [n for n in names if not n.startswith('h=')]
ims = []
for n in names:
    i = Image.open(f'shots/{n}.png').convert('RGB')
    ims.append(i.resize((int(i.width * H / i.height), H), Image.LANCZOS))
W = sum(i.width for i in ims) + 10 * (len(ims) + 1)
s = Image.new('RGB', (W, H + 20), (40, 42, 50))
x = 10
for i in ims:
    s.paste(i, (x, 10))
    x += i.width + 10
s.save(out)
