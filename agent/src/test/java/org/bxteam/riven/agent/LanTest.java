package org.bxteam.riven.agent;

import static org.junit.jupiter.api.Assertions.*;

import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.MulticastSocket;
import java.net.NetworkInterface;
import java.nio.charset.StandardCharsets;
import java.util.Collections;
import org.junit.jupiter.api.Test;

class LanTest {
    @Test
    void theDetectorHearsLoopbackAndStillTimesOut() throws Exception {
        Class<?> type = new Patched().loadClass(LanDetector.class.getName());
        Object detector = type.getConstructor().newInstance();
        try {
            int port = (int) type.getMethod("port").invoke(detector);
            String world = "[MOTD]Alex - World[/MOTD][AD]41234[/AD]";
            try (MulticastSocket riven = new MulticastSocket(new InetSocketAddress(InetAddress.getLoopbackAddress(), 0))) {
                riven.setNetworkInterface(NetworkInterface.getByInetAddress(InetAddress.getLoopbackAddress()));
                byte[] bytes = world.getBytes(StandardCharsets.UTF_8);
                riven.send(new DatagramPacket(bytes, bytes.length, InetAddress.getByName(Lan.GROUP), port));
            }
            assertEquals(world + "@127.0.0.1", type.getMethod("next").invoke(detector));
            // The game's loop catches the timeout by its type; a wrapped one would end LAN detection for good.
            assertNull(type.getMethod("next").invoke(detector));
        } finally {
            type.getMethod("close").invoke(detector);
        }
    }

    @Test
    void anAnnouncementRepeatedFromAnotherLocalAddressIsDropped() throws Exception {
        InetAddress other = Collections.list(NetworkInterface.getNetworkInterfaces()).stream()
                .flatMap(i -> Collections.list(i.getInetAddresses()).stream())
                .filter(a -> !a.isLoopbackAddress() && a.getAddress().length == 4)
                .findFirst()
                .orElse(null);
        byte[] world = "[MOTD]Steve - Base[/MOTD][AD]40001[/AD]".getBytes(StandardCharsets.UTF_8);
        assertFalse(Lan.repeated(packet(world, InetAddress.getLoopbackAddress())));
        assertFalse(Lan.repeated(packet(world, InetAddress.getLoopbackAddress())));
        if (other != null) assertTrue(Lan.repeated(packet(world, other)));
        // Another computer with the same world name is a different world.
        assertFalse(Lan.repeated(packet(world, InetAddress.getByName("203.0.113.7"))));
    }

    private static DatagramPacket packet(byte[] bytes, InetAddress from) {
        return new DatagramPacket(bytes, bytes.length, from, 4445);
    }

    /** Loads the detector patched as the agent would, from a loader that cannot see the agent's classes. */
    private static final class Patched extends ClassLoader {
        Patched() {
            super(ClassLoader.getPlatformClassLoader());
        }

        @Override
        protected Class<?> findClass(String name) throws ClassNotFoundException {
            try (InputStream in = LanTest.class.getResourceAsStream("/" + name.replace('.', '/') + ".class")) {
                if (in == null) throw new ClassNotFoundException(name);
                byte[] patched = Lan.apply(in.readAllBytes());
                assertNotNull(patched, "the detector was not recognised");
                return defineClass(name, patched, 0, patched.length);
            } catch (java.io.IOException e) {
                throw new ClassNotFoundException(name, e);
            }
        }
    }
}
