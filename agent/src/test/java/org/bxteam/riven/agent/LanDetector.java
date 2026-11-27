package org.bxteam.riven.agent;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.InetAddress;
import java.net.MulticastSocket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;

/** The shape of the game's LAN detector, on a free port: what `Lan.apply` patches. */
public class LanDetector {
    private final MulticastSocket socket;

    public LanDetector() throws IOException {
        socket = new MulticastSocket(0);
        socket.setSoTimeout(1500);
        socket.joinGroup(InetAddress.getByName("224.0.2.60"));
    }

    public int port() {
        return socket.getLocalPort();
    }

    /** The next announcement as `text@address`, or null once the socket times out, as the game's loop goes on. */
    public String next() throws IOException {
        byte[] buffer = new byte[1024];
        DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
        try {
            socket.receive(packet);
        } catch (SocketTimeoutException e) {
            return null;
        }
        return new String(packet.getData(), packet.getOffset(), packet.getLength(), StandardCharsets.UTF_8)
                + "@" + packet.getAddress().getHostAddress();
    }

    public void close() {
        socket.close();
    }
}
