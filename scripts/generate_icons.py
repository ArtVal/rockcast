from PIL import Image, ImageDraw

def create_search():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # Circle
    d.ellipse([40, 40, 160, 160], outline=(243, 238, 233, 230), width=24)
    # Handle
    d.line([(145, 145), (216, 216)], fill=(243, 238, 233, 230), width=28)
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_search.png")

def create_mic():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # Same near-white as the other glyphs so ACCENT tinting works (orange
    # fill would multiply with the orange tint into an unreadable blob).
    color = (243, 238, 233, 230)
    # Capsule
    d.rounded_rectangle([96, 28, 160, 140], radius=32, fill=color)
    # U-arc
    d.arc([64, 56, 192, 168], start=0, end=180, fill=color, width=20)
    # Stem and base (rounded joints)
    d.line([(128, 168), (128, 216)], fill=color, width=22, joint="curve")
    d.line([(84, 220), (172, 220)], fill=color, width=22, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_mic.png")

def create_speaker():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # Box
    d.rectangle([40, 90, 96, 166], fill=(243, 238, 233, 220))
    # Cone
    d.polygon([(96, 90), (160, 46), (160, 210), (96, 166)], fill=(243, 238, 233, 220))
    # Sound wave arc
    d.arc([140, 70, 210, 186], start=-60, end=60, fill=(243, 238, 233, 220), width=18)
    d.arc([160, 40, 246, 216], start=-60, end=60, fill=(243, 238, 233, 220), width=18)
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_speaker.png")

def create_speaker_mute():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # Box
    d.rectangle([40, 90, 96, 166], fill=(158, 141, 130, 200))
    # Cone
    d.polygon([(96, 90), (160, 46), (160, 210), (96, 166)], fill=(158, 141, 130, 200))
    # X cross
    d.line([(180, 100), (230, 156)], fill=(220, 38, 38, 240), width=22)
    d.line([(230, 100), (180, 156)], fill=(220, 38, 38, 240), width=22)
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_speaker_mute.png")

def create_clear():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    color = (243, 238, 233, 230)
    # Cross (clear input)
    d.line([(72, 72), (184, 184)], fill=color, width=28, joint="curve")
    d.line([(184, 72), (72, 184)], fill=color, width=28, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_clear.png")

def create_pc():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    color = (243, 238, 233, 230)
    # Screen
    d.rounded_rectangle([36, 52, 220, 168], radius=14, outline=color, width=18)
    # Stand
    d.line([(128, 172), (128, 204)], fill=color, width=20, joint="curve")
    d.line([(84, 210), (172, 210)], fill=color, width=20, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_pc.png")

def create_phone():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    color = (243, 238, 233, 230)
    # Body
    d.rounded_rectangle([76, 28, 180, 228], radius=22, outline=color, width=18)
    # Home line
    d.line([(112, 196), (144, 196)], fill=color, width=16, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_phone.png")

def create_logo():
    # Brand mark for the app header, derived from the master app icon.
    # Multicolor art: drawn untinted, unlike the monochrome glyph set.
    im = Image.open("assets/app-icon.png").convert("RGBA")
    im = im.resize((128, 128), Image.Resampling.LANCZOS)
    im.save("assets/icon_logo.png")


def create_ico():
    # Multi-resolution Windows icon embedded into the .exe by build.rs.
    im = Image.open("assets/app-icon.png").convert("RGBA")
    im.save(
        "assets/app-icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


def create_arrow_up():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    color = (243, 238, 233, 230)
    d.line([(128, 60), (128, 204)], fill=color, width=24, joint="curve")
    d.line([(64, 124), (128, 60), (192, 124)], fill=color, width=24, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_arrow_up.png")

def create_target():
    im = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    color = (243, 238, 233, 230)
    d.ellipse([60, 60, 196, 196], outline=color, width=20)
    d.ellipse([114, 114, 142, 142], fill=color)
    d.line([(128, 32), (128, 68)], fill=color, width=20, joint="curve")
    d.line([(128, 188), (128, 224)], fill=color, width=20, joint="curve")
    d.line([(32, 128), (68, 128)], fill=color, width=20, joint="curve")
    d.line([(188, 128), (224, 128)], fill=color, width=20, joint="curve")
    im = im.resize((64, 64), Image.Resampling.LANCZOS)
    im.save("assets/icon_target.png")

create_search()
create_mic()
create_speaker()
create_speaker_mute()
create_clear()
create_pc()
create_phone()
create_logo()
create_ico()
create_arrow_up()
create_target()
print("All icons generated successfully!")
