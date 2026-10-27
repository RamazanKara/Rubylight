import java.awt.*;
import java.awt.geom.*;
import java.awt.image.BufferedImage;
import java.awt.image.DataBufferByte;
import java.io.*;
import java.nio.file.*;
import java.util.Random;
import javax.imageio.ImageIO;

public class GenerateAssets {
    static final Color ROSE = new Color(255, 64, 108), INK = new Color(17, 26, 31);
    static final String[] TITLES = {"Desktop", "Racing Game", "Open World", "Space Sim", "Puzzle Night",
            "Retro Arcade", "Strategy", "Music Studio", "Ocean Explorer", "Skybound"};
    static final String[] GLYPHS = {"D", "R", "O", "S", "P", "A", "S", "M", "O", "S"};
    // Each cover names a genre, not the app, so the art reads as a game library on any client.
    static final String[] GENRES = {"FULL DESKTOP", "ARCADE RACING", "ADVENTURE", "SIMULATION", "PUZZLE",
            "CLASSIC ARCADE", "REAL-TIME STRATEGY", "MUSIC CREATION", "EXPLORATION", "FLIGHT ACTION"};
    static final Color[] ACCENTS = {ROSE, new Color(255, 122, 81), new Color(108, 226, 187),
            new Color(147, 135, 255), new Color(247, 199, 108), ROSE, new Color(109, 197, 228),
            new Color(197, 126, 227), new Color(62, 208, 209), new Color(249, 136, 156)};

    static Graphics2D graphics(BufferedImage image) {
        Graphics2D g = image.createGraphics();
        g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON);
        g.setRenderingHint(RenderingHints.KEY_TEXT_ANTIALIASING, RenderingHints.VALUE_TEXT_ANTIALIAS_ON);
        return g;
    }

    static void text(Graphics2D g, String text, float size, int x, int y, Color color) {
        g.setFont(new Font("Segoe UI", Font.BOLD, 1).deriveFont(size));
        g.setColor(color);
        g.drawString(text, x, y);
    }

    static void glow(Graphics2D g, float x, float y, float radius, Color color) {
        g.setPaint(new RadialGradientPaint(x, y, radius, new float[] {0, 1},
                new Color[] {color, new Color(color.getRed(), color.getGreen(), color.getBlue(), 0)}));
        g.fill(new Ellipse2D.Float(x - radius, y - radius, radius * 2, radius * 2));
    }

    // Covers are 600x800 unless listed: a few differ so every client shows how a banner, a square and a 2:3 poster fit.
    static int[] artSize(int index) {
        switch (index) {
            case 1: return new int[] {920, 430};  // landscape banner, like a Steam header
            case 2: return new int[] {600, 600};  // square
            case 3: return new int[] {600, 900};  // 2:3 poster
            default: return new int[] {600, 800};
        }
    }

    static void art(Path directory) throws IOException {
        Files.createDirectories(directory);
        for (int index = 0; index < TITLES.length; index++) {
            int[] dims = artSize(index);
            BufferedImage image = new BufferedImage(dims[0], dims[1], BufferedImage.TYPE_INT_RGB);
            Graphics2D g = graphics(image);
            // The 600x800 composition is scaled to fit and bottom-aligned; the background runs on past its edges.
            double fit = Math.min(dims[0] / 600.0, dims[1] / 800.0);
            g.translate((dims[0] - 600 * fit) / 2, dims[1] - 800 * fit);
            g.scale(fit, fit);
            Color accent = ACCENTS[index];
            g.setPaint(new GradientPaint(0, 800, INK, 600, 0,
                    new Color(accent.getRed() / 3, accent.getGreen() / 3, accent.getBlue() / 3)));
            g.fillRect(-2000, -2000, 5000, 5000);
            glow(g, 440, 190, 400, new Color(accent.getRed(), accent.getGreen(), accent.getBlue(), 95));
            Random random = new Random(810 + index);
            for (int p = 0; p < 90; p++) {
                g.setColor(new Color(240, 243, 244, random.nextInt(100) + 30));
                int size = random.nextInt(3) + 1;
                g.fillOval(random.nextInt(600), random.nextInt(600), size, size);
            }
            g.setStroke(new BasicStroke(2));
            g.setColor(new Color(accent.getRed(), accent.getGreen(), accent.getBlue(), 75));
            for (int ring = 0; ring < 5; ring++) {
                int radius = 105 + ring * 44;
                g.drawOval(300 - radius, 335 - radius, radius * 2, radius * 2);
            }
            AffineTransform original = g.getTransform();
            g.translate(300, 335);
            g.rotate((index - 4) * 0.13);
            g.setColor(new Color(accent.getRed(), accent.getGreen(), accent.getBlue(), 28));
            g.fillRoundRect(-132, -145, 264, 290, 35, 35);
            g.setStroke(new BasicStroke(4));
            g.setColor(accent);
            g.drawRoundRect(-132, -145, 264, 290, 35, 35);
            text(g, GLYPHS[index], 182, -66, 65, new Color(240, 243, 244));
            g.setTransform(original);
            g.setPaint(new GradientPaint(0, 520, new Color(17, 26, 31, 0), 0, 800, INK));
            g.fillRect(-2000, 520, 5000, 2280);
            text(g, String.format("%02d  /  COLLECTION", index + 1), 16, 40, 578, accent);
            String[] words = TITLES[index].toUpperCase(java.util.Locale.ROOT).split(" ");
            for (int word = 0; word < words.length; word++) text(g, words[word], 52, 38, 643 + word * 58, Color.WHITE);
            g.setColor(accent);
            g.fillRoundRect(40, 751, 52, 5, 3, 3);
            text(g, GENRES[index], 14, 112, 759, new Color(188, 200, 206));
            g.dispose();
            ImageIO.write(image, "png", directory.resolve((index + 1) + ".png").toFile());
        }
    }

    static BufferedImage background() {
        BufferedImage image = new BufferedImage(1920, 1080, BufferedImage.TYPE_3BYTE_BGR);
        Graphics2D g = graphics(image);
        g.setPaint(new GradientPaint(0, 0, new Color(9, 14, 35), 0, 860, new Color(89, 36, 74)));
        g.fillRect(0, 0, 1920, 1080);
        glow(g, 1280, 370, 460, new Color(255, 64, 108, 105));
        Random random = new Random(54);
        for (int i = 0; i < 350; i++) {
            g.setColor(new Color(200, 216, 255, random.nextInt(150) + 60));
            int size = random.nextInt(3) + 1;
            g.fillOval(random.nextInt(1920), random.nextInt(700), size, size);
        }
        g.setPaint(new GradientPaint(1180, 170, new Color(255, 170, 169), 1400, 560, new Color(111, 45, 92)));
        g.fillOval(1110, 155, 350, 350);
        g.setColor(new Color(255, 224, 213, 100));
        g.setStroke(new BasicStroke(4));
        g.rotate(-0.23, 1285, 330);
        g.drawOval(990, 280, 590, 100);
        g.dispose();
        return image;
    }

    static void frame(Graphics2D g, BufferedImage background, int frame) {
        double t = frame / 60.0, phase = t * Math.PI / 5;
        g.drawImage(background, 0, 0, null);
        for (int layer = 0; layer < 4; layer++) {
            Path2D ridge = new Path2D.Double();
            ridge.moveTo(0, 1080);
            for (int x = 0; x <= 1940; x += 20) {
                double world = x / 220.0 + phase * (layer + 1);
                double y = 520 + layer * 72 - Math.abs(Math.sin(world) * 90 + Math.sin(world * 3) * 35);
                ridge.lineTo(x, y);
            }
            ridge.lineTo(1940, 1080);
            ridge.closePath();
            g.setColor(new Color(45 - layer * 8, 40 - layer * 5, 69 - layer * 9));
            g.fill(ridge);
            g.setColor(new Color(255, 89, 129, 95 - layer * 17));
            g.setStroke(new BasicStroke(2));
            g.draw(ridge);
        }
        g.setPaint(new GradientPaint(0, 695, new Color(19, 30, 47), 0, 1080, new Color(6, 13, 21)));
        g.fillRect(0, 695, 1920, 385);
        g.setStroke(new BasicStroke(2));
        for (int x = -15; x <= 15; x++) {
            g.setColor(new Color(64, 135, 163, 60));
            g.drawLine(960 + x * 30, 695, 960 + x * 220, 1080);
        }
        for (int i = 0; i < 22; i++) {
            double z = ((i / 22.0 + t / 5) % 1);
            int y = 695 + (int) (z * z * 385);
            g.setColor(new Color(86, 172, 192, (int) (40 + z * 65)));
            g.drawLine(0, y, 1920, y);
        }
        for (int i = 0; i < 28; i++) {
            double z = ((i / 28.0 + t / 10) % 1);
            double scale = 0.15 + z * z * 1.8;
            int side = i % 2 == 0 ? -1 : 1;
            int x = 960 + (int) (side * (170 + z * 890));
            int base = 675 + (int) (z * z * 500);
            int width = (int) (45 * scale), height = (int) ((130 + i % 4 * 30) * scale);
            g.setColor(new Color(19, 28, 43));
            g.fillRect(x, base - height, width, height);
            g.setColor(i % 3 == 0 ? ROSE : new Color(76, 189, 204));
            g.fillRect(x, base - height, Math.max(2, width / 9), height);
            g.setColor(new Color(232, 146, 165, 150));
            g.fillRect(x, base - height, width, 3);
        }
        Random random = new Random(21);
        for (int p = 0; p < 130; p++) {
            double start = random.nextDouble(), speed = 1 + random.nextInt(3);
            int x = (int) ((random.nextDouble() * 1920 + t * 192 * speed) % 1920);
            int y = (int) (400 + (start * 640 + 65 * Math.sin(phase + p)) % 640);
            g.setColor(new Color(255, 177, 162, 80 + p % 100));
            g.fillOval(x, y, 2 + p % 4, 2 + p % 4);
        }
        int shipX = 960 + (int) (Math.sin(phase) * 190), shipY = 782 + (int) (Math.sin(phase * 2) * 25);
        glow(g, shipX, shipY + 40, 155, new Color(255, 64, 108, 145));
        g.setColor(new Color(255, 154, 112, 190));
        g.fill(new Polygon(new int[] {shipX - 38, shipX, shipX + 38},
                new int[] {shipY + 20, shipY + 150 + (int) (Math.sin(phase * 12) * 18), shipY + 20}, 3));
        g.setColor(new Color(23, 35, 53));
        Polygon hull = new Polygon(new int[] {shipX, shipX + 120, shipX + 34, shipX, shipX - 34, shipX - 120},
                new int[] {shipY - 100, shipY + 38, shipY + 20, shipY + 46, shipY + 20, shipY + 38}, 6);
        g.fill(hull);
        g.setColor(new Color(179, 231, 234));
        g.setStroke(new BasicStroke(3));
        g.draw(hull);
        g.setColor(ROSE);
        g.fill(new Polygon(new int[] {shipX, shipX + 20, shipX - 20}, new int[] {shipY - 68, shipY + 8, shipY + 8}, 3));
        // The top panels sit 112 px down: the client's performance overlay takes the top corner of the screen.
        g.setColor(new Color(10, 18, 28, 195));
        g.fillRoundRect(70, 112, 420, 122, 20, 20);
        text(g, "SKYBOUND", 35, 96, 159, Color.WHITE);
        text(g, "SECTOR 07  /  OUTER RIM", 18, 98, 199, new Color(138, 208, 217));
        g.setColor(new Color(10, 18, 28, 195));
        g.fillRoundRect(1440, 113, 390, 115, 20, 20);
        text(g, "CRUISE", 17, 1465, 149, new Color(188, 200, 206));
        text(g, String.format(java.util.Locale.ROOT, "%03d  KM/H", 280 + (int) (18 * Math.sin(phase))), 35,
                1465, 197, Color.WHITE);
        g.setColor(new Color(11, 22, 31, 210));
        g.fillRoundRect(70, 880, 385, 110, 20, 20);
        text(g, "SHIELD", 17, 94, 914, new Color(188, 200, 206));
        g.setColor(new Color(57, 78, 86));
        g.fillRoundRect(96, 936, 330, 12, 6, 6);
        g.setColor(ROSE);
        g.fillRoundRect(96, 936, 285 + (int) (18 * Math.sin(phase)), 12, 6, 6);
        g.setColor(new Color(138, 225, 224, 120));
        g.setStroke(new BasicStroke(2));
        int aimX = 960 + (int) (Math.sin(phase + 0.5) * 105), aimY = 505 + (int) (Math.cos(phase) * 25);
        g.drawOval(aimX - 32, aimY - 32, 64, 64);
        g.drawLine(aimX - 48, aimY, aimX - 22, aimY);
        g.drawLine(aimX + 22, aimY, aimX + 48, aimY);
        g.drawLine(aimX, aimY - 48, aimX, aimY - 22);
        g.setColor(new Color(10, 18, 28, 180));
        g.fillOval(1630, 830, 175, 175);
        g.setColor(new Color(110, 190, 192, 140));
        g.drawOval(1630, 830, 175, 175);
        g.drawOval(1674, 874, 87, 87);
        g.drawLine(1717, 917, 1717 + (int) (Math.cos(phase) * 84), 917 + (int) (Math.sin(phase) * 84));
        g.setColor(ROSE);
        g.fillOval(1680 + (int) (Math.sin(phase) * 20), 888, 9, 9);
    }

    public static void main(String[] args) throws Exception {
        Path assets = Path.of(args[0]);
        art(assets.resolve("art"));
        if (args.length > 2 && args[2].equals("--art-only")) return;
        Process encoder = new ProcessBuilder(args[1], "-y", "-hide_banner", "-loglevel", "warning",
                "-f", "rawvideo", "-pixel_format", "bgr24", "-video_size", "1920x1080", "-framerate", "60",
                "-i", "pipe:0", "-an", "-c:v", "libx264", "-preset", "fast", "-crf", "19", "-threads", "2",
                "-pix_fmt", "yuv420p", "-movflags", "+faststart", assets.resolve("stream-h264.mp4").toString())
                .redirectError(ProcessBuilder.Redirect.INHERIT).start();
        BufferedImage background = background();
        BufferedImage image = new BufferedImage(1920, 1080, BufferedImage.TYPE_3BYTE_BGR);
        try (OutputStream pipe = new BufferedOutputStream(encoder.getOutputStream(), 1024 * 1024)) {
            for (int f = 0; f < 600; f++) {
                Graphics2D g = graphics(image);
                frame(g, background, f);
                g.dispose();
                if (f == 150) ImageIO.write(image, "png", Path.of(args[2]).toFile());
                pipe.write(((DataBufferByte) image.getRaster().getDataBuffer()).getData());
                if (f % 120 == 0) System.out.println("Rendered " + f + " / 600 frames");
            }
        }
        if (encoder.waitFor() != 0) throw new IOException("FFmpeg failed");
    }
}
