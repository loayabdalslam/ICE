"""Generate editable vector identity assets and previews of the actual Ratatui buffer."""
from pathlib import Path
import json
from PIL import Image, ImageDraw, ImageFont

out = Path('brand')
out.mkdir(exist_ok=True)
bg, surface, accent, ice, text, muted = '#061018', '#0a1c28', '#50d2ff', '#a0ecff', '#e2f4fc', '#6e9cb0'
mark = '''<path d="M32 72 64 40H152L184 72 152 104H32Z" fill="#a0ecff"/>
<path d="M152 104 184 72V160L152 192Z" fill="#2078a0"/>
<path d="M32 104H152V192H32Z" fill="#50d2ff"/>
<path d="M44 112H52V132H44Z" fill="#e2f4fc"/>
<path d="M66 136H78V150H66ZM114 136H126V150H114ZM88 168H106V174H88Z" fill="#061018"/>'''
def svg(w, h, content):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{content}</svg>'

(out/'floe.svg').write_text(svg(216,216,mark), encoding='utf-8')
(out/'logo.svg').write_text(svg(640,216,mark+f'<text x="238" y="161" font-family="Consolas, monospace" font-size="126" font-weight="700" fill="{text}">ICE</text>'), encoding='utf-8')
(out/'logo-mono.svg').write_text(svg(640,216,mark.replace('#a0ecff',text).replace('#2078a0',text).replace('#50d2ff',text)+f'<text x="238" y="161" font-family="Consolas, monospace" font-size="126" font-weight="700" fill="{text}">ICE</text>'), encoding='utf-8')
font_path = 'C:/Windows/Fonts/consola.ttf'
font = ImageFont.truetype(font_path, 16)
title_font = ImageFont.truetype('C:/Windows/Fonts/consolab.ttf', 70)
small = ImageFont.truetype(font_path, 20)
icon = Image.new('RGBA',(256,256),(0,0,0,0))
d = ImageDraw.Draw(icon)
d.polygon([(40,88),(80,48),(184,48),(216,88),(176,128),(40,128)], fill=ice)
d.polygon([(176,128),(216,88),(216,192),(176,224)], fill='#2078a0')
d.rectangle((40,128,175,223), fill=accent)
d.rectangle((52,140,60,158),fill=text)
for x in [78,132]: d.rectangle((x,164,x+13,180),fill=bg)
d.rectangle((104,200,122,206),fill=bg)
icon.save(out/'floe.png')
icon.save(out/'ice.ico',sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])

def render(cells):
    im=Image.new('RGB',(120*10+48,40*20+76),bg)
    dr=ImageDraw.Draw(im)
    dr.rounded_rectangle((8,8,1239,867),radius=14,outline='#1c485c',width=1)
    for x,c in [(30,'#ff6078'),(50,'#f0c450'),(70,'#40dcaa')]: dr.ellipse((x,24,x+8,32),fill=c)
    dr.text((98,18),'ICE / terminal',font=font,fill=muted)
    for i,(symbol,fg,cell_bg) in enumerate(cells):
        x,y=24+i%120*10,52+i//120*20
        if cell_bg!=bg:dr.rectangle((x,y,x+9,y+19),fill=cell_bg)
        if symbol.strip(): dr.text((x,y-1),symbol,font=font,fill=fg,stroke_width=0)
    return im

captures=out/'captures'
for name in ['welcome','connect','session']:
    frames=json.loads((captures/f'{name}.json').read_text(encoding='utf-8'))
    images=[render(f) for f in frames]
    images[0].save(out/f'{name}.png')
    if name=='welcome':images[0].save(out/'ice-preview.gif',save_all=True,append_images=images[1:],duration=160,loop=0)

board=Image.new('RGB',(1600,1100),bg)
d=ImageDraw.Draw(board)
d.text((64,42),'ICE / IDENTITY SYSTEM',font=small,fill=muted)
d.text((64,91),'Clarity in motion.',font=title_font,fill=text)
d.text((64,194),'Intent. Compile. Execute.',font=small,fill=ice)
board.paste(icon.resize((260,260)),(1190,38),icon.resize((260,260)))
preview=Image.open(out/'welcome.png'); preview.thumbnail((1000,700)); board.paste(preview,(48,330))
d.text((1100,352),'01 / GLACIER PALETTE',font=small,fill=text)
for i,(label,color) in enumerate([('MIDNIGHT',bg),('SURFACE',surface),('GLACIER',accent),('FROST',ice),('SNOW',text)]):
    y=403+i*62;d.rounded_rectangle((1100,y,1150,y+40),radius=4,fill=color,outline=muted)
    d.text((1170,y+5),f'{label} {color}',font=font,fill=muted)
d.text((1100,762),'02 / FLOE',font=small,fill=text)
d.text((1100,806),'A curious ice companion.\nPixel geometry. Three faces.\nQuiet motion. A little soul.',font=font,fill=muted,spacing=10)
d.text((64,1020),'MONOSPACE FIRST    /    PRECISE, CALM, HUMAN    /    ICE 0.2',font=small,fill=muted)
board.save(out/'identity-board.png')

(out/'preview.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ICE — Clarity in motion</title>
<style>body{margin:0;background:#061018;color:#e2f4fc;font:16px Consolas,monospace;padding:40px}header,main{max-width:1248px;margin:auto}h1{font-size:42px;margin:14px 0}p{color:#6e9cb0}nav{display:flex;gap:12px;margin:28px 0;flex-wrap:wrap}button{background:#0a1c28;color:#a0ecff;border:1px solid #1c485c;padding:12px 22px;border-radius:8px;cursor:pointer}button[aria-pressed=true]{background:#50d2ff;color:#061018}img{width:100%;border-radius:14px}footer{margin:28px 0;color:#6e9cb0}</style>
<header><p>ICE / 0.2 / FLOE EDITION</p><h1>Clarity in motion.</h1><p>Actual TUI render captures. Sample conversation for visual review.</p><nav><button data-src="ice-preview.gif" aria-pressed="true">Welcome · animated</button><button data-src="connect.png">Connect</button><button data-src="session.png">Conversation</button><button data-src="identity-board.png">Brand identity</button><button id="motion">Pause motion</button></nav></header><main><img id="preview" src="ice-preview.gif" alt="ICE terminal interface"><footer>Intent. Compile. Execute. / Terminal-native pixel companion.</footer></main>
<script>const preview=document.getElementById('preview');for(const button of document.querySelectorAll('[data-src]'))button.onclick=()=>{preview.src=button.dataset.src;for(const b of document.querySelectorAll('[data-src]'))b.setAttribute('aria-pressed',b===button)};document.getElementById('motion').onclick=()=>{const paused=preview.getAttribute('src')==='welcome.png';preview.src=paused?'ice-preview.gif':'welcome.png';document.getElementById('motion').textContent=paused?'Pause motion':'Play motion'};if(matchMedia('(prefers-reduced-motion: reduce)').matches)preview.src='welcome.png';</script></html>''',encoding='utf-8')

