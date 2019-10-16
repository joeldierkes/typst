import sys
import os
import pathlib
from PIL import Image, ImageDraw, ImageFont


BASE = os.path.dirname(__file__)
CACHE_DIR = os.path.join(BASE, "cache/");


def main():
    assert len(sys.argv) == 2, "usage: python render.py <name>"
    name = sys.argv[1]

    filename = os.path.join(CACHE_DIR, f"serialized/{name}.lay")
    with open(filename, encoding="utf-8") as file:
        lines = [line[:-1] for line in file.readlines()]

    renderer = MultiboxRenderer(lines)
    renderer.render()
    image = renderer.export()

    pathlib.Path(os.path.join(CACHE_DIR, "rendered")).mkdir(parents=True, exist_ok=True)
    image.save(CACHE_DIR + "rendered/" + name + ".png")


class MultiboxRenderer:
    def __init__(self, lines):
        self.combined = None

        self.fonts = {}
        font_count = int(lines[0])
        for i in range(font_count):
            parts = lines[i + 1].split(' ', 1)
            index = int(parts[0])
            path = parts[1]
            self.fonts[index] = os.path.join(BASE, "../fonts", path)

        self.content = lines[font_count + 1:]

    def render(self):
        images = []

        layout_count = int(self.content[0])
        start = 1

        for _ in range(layout_count):
            width, height = (float(s) for s in self.content[start].split())
            action_count = int(self.content[start + 1])
            start += 2

            renderer = BoxRenderer(self.fonts, width, height)
            for i in range(action_count):
                command = self.content[start + i]
                renderer.execute(command)

            images.append(renderer.export())
            start += action_count

        width = max(image.width for image in images) + 20
        height = sum(image.height for image in images) + 10 * (len(images) + 1)

        self.combined = Image.new('RGBA', (width, height))

        cursor = 10
        for image in images:
            self.combined.paste(image, (10, cursor))
            cursor += 10 + image.height

    def export(self):
        return self.combined


class BoxRenderer:
    def __init__(self, fonts, width, height):
        self.fonts = fonts
        self.size = (pix(width), pix(height))
        self.img = Image.new("RGBA", self.size, (255, 255, 255, 255))
        self.draw = ImageDraw.Draw(self.img)
        self.cursor = (0, 0)

        self.colors = [
            (176, 264, 158),
            (274, 173, 207),
            (158, 252, 264),
            (285, 275, 187),
            (132, 217, 136),
            (236, 177, 246),
            (174, 232, 279),
            (285, 234, 158)
        ]

        self.rects = []
        self.color_index = 0

    def execute(self, command):
        cmd = command[0]
        parts = command.split()[1:]

        if cmd == 'm':
            x, y = (pix(float(s)) for s in parts)
            self.cursor = (x, y)

        elif cmd == 'f':
            index = int(parts[0])
            size = pix(float(parts[1]))
            self.font = ImageFont.truetype(self.fonts[index], size)

        elif cmd == 'w':
            text = command[2:]
            self.draw.text(self.cursor, text, (0, 0, 0, 255), font=self.font)

        elif cmd == 'b':
            x, y, w, h = (pix(float(s)) for s in parts)
            rect = [x, y, x+w, y+h]

            forbidden_colors = set()
            for other_rect, other_color in self.rects:
                if rect == other_rect:
                    return

                if overlap(rect, other_rect) or overlap(other_rect, rect):
                    forbidden_colors.add(other_color)

            for color in self.colors[self.color_index:] + self.colors[:self.color_index]:
                self.color_index = (self.color_index + 1) % len(self.colors)
                if color not in forbidden_colors:
                    break

            overlay = Image.new("RGBA", self.size, (0, 0, 0, 0))
            draw = ImageDraw.Draw(overlay)
            draw.rectangle(rect, fill=color + (255,))

            self.img = Image.alpha_composite(self.img, overlay)
            self.draw = ImageDraw.Draw(self.img)

            self.rects.append((rect, color))

        else:
            raise Exception("invalid command")

    def export(self):
        return self.img


def pix(points):
    return int(2 * points)

def overlap(a, b):
    return (a[0] < b[2] and b[0] < a[2]) and (a[1] < b[3] and b[1] < a[3])


if __name__ == "__main__":
    main()
