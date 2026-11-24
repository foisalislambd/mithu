#!/usr/bin/env python3
"""Convert Chromium vector icons (.icon path text) into SVG files.

Usage: tools/icon2svg.py <file.icon>... <out-dir>

Each CANVAS_DIMENSIONS representation becomes <name>_<size>.svg, with the
`_chrome_refresh_old`, `_refresh_old` and `_old` suffixes dropped. Commands are
mapped 1:1 (MOVE/LINE/H/V/CUBIC/QUAD/ARC, absolute and relative, CIRCLE,
ROUND_RECT, STROKE, FILL_RULE_*). The .icon sources are BSD-3-Clause (The
Chromium Authors); see crates/ui/icons/NOTICE.
"""
import re,sys,os
def parse(text):
    text=re.sub(r'//.*','',text)
    toks=[t for t in re.split(r'[,\s]+',text) if t]
    reps=[];cur=None;paths=None
    i=0
    def num():
        nonlocal i
        v=toks[i]; i+=1
        return float(v.rstrip('f'))
    path=[];allp=[];size=None;rule='nonzero';stroke=None;cap='round'
    def flush():
        nonlocal path
        if path: allp.append((' '.join(path),rule,stroke,cap))
        path=[]
    while i<len(toks):
        t=toks[i];i+=1
        if t=='CANVAS_DIMENSIONS':
            if size is not None:
                flush(); reps.append((size,allp)); allp=[]
            size=num(); rule='nonzero'; stroke=None
        elif t=='NEW_PATH': flush(); stroke=None
        elif t=='FILL_RULE_NONZERO': rule='nonzero'
        elif t=='FILL_RULE_EVENODD': rule='evenodd'
        elif t=='STROKE': stroke=num()
        elif t=='CAP_SQUARE': cap='square'
        elif t=='CAP_ROUND': cap='round'
        elif t in ('PATH_COLOR_ARGB',): [num() for _ in range(4)]
        elif t in ('PATH_MODE_CLEAR','DISABLE_AA','END','PATH_COLOR_ALPHA','FLIPS_IN_RTL'):
            if t=='PATH_COLOR_ALPHA': num()
        else:
            m={'MOVE_TO':('M',2),'R_MOVE_TO':('m',2),'LINE_TO':('L',2),'R_LINE_TO':('l',2),'H_LINE_TO':('H',1),'R_H_LINE_TO':('h',1),'V_LINE_TO':('V',1),'R_V_LINE_TO':('v',1),'CUBIC_TO':('C',6),'R_CUBIC_TO':('c',6),'CUBIC_TO_SHORTHAND':('S',4),'R_CUBIC_TO_SHORTHAND':('s',4),'QUADRATIC_TO':('Q',4),'R_QUADRATIC_TO':('q',4),'QUADRATIC_TO_SHORTHAND':('T',2),'R_QUADRATIC_TO_SHORTHAND':('t',2),'ARC_TO':('A',7),'R_ARC_TO':('a',7),'CLOSE':('Z',0)}
            if t in m:
                c,n=m[t]; args=[num() for _ in range(n)]
                path.append(c+' '+' '.join('%g'%a for a in args))
            elif t=='CIRCLE':
                cx,cy,r=num(),num(),num()
                path.append(f'M {cx-r} {cy} a {r} {r} 0 1 0 {2*r} 0 a {r} {r} 0 1 0 {-2*r} 0 Z')
            elif t=='ROUND_RECT':
                x,y,w,h,r=[num() for _ in range(5)]
                path.append(f'M {x+r} {y} h {w-2*r} a {r} {r} 0 0 1 {r} {r} v {h-2*r} a {r} {r} 0 0 1 {-r} {r} h {-(w-2*r)} a {r} {r} 0 0 1 {-r} {-r} v {-(h-2*r)} a {r} {r} 0 0 1 {r} {-r} Z')
            else:
                raise SystemExit(f'unknown {t}')
    flush()
    if size is None: size=48
    reps.append((size,allp))
    return reps
src,out=sys.argv[1],sys.argv[2]
for f in sorted(os.listdir(src)):
    if not f.endswith('.icon'): continue
    reps=parse(open(os.path.join(src,f)).read())
    for size,ps in reps:
        body=''
        for d,rule,stroke,cap in ps:
            if stroke: body+=f'<path d="{d}" fill="none" stroke="currentColor" stroke-width="{stroke}" stroke-linecap="{cap}"/>'
            else: body+=f'<path d="{d}" fill="currentColor" fill-rule="{rule}"/>'
        name=f[:-5].replace('_chrome_refresh_old','').replace('_refresh_old','').replace('_old','')
        s=int(size)
        open(os.path.join(out,f'{name}_{s}.svg'),'w').write(f'<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">{body}</svg>\n')
        print(name,s)
