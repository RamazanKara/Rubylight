import java.awt.*;
import java.awt.geom.*;
import java.awt.image.BufferedImage;
import java.awt.image.DataBufferByte;
import java.io.*;
import java.nio.file.*;
import java.util.Random;
import javax.imageio.ImageIO;

public class GenerateAssets {
    static final Color ROSE = new Color(255, 64, 108);

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
