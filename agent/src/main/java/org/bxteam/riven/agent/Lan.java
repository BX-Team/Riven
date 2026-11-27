package org.bxteam.riven.agent;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.MulticastSocket;
import java.net.NetworkInterface;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Lets the game's LAN list hear worlds Riven announces on loopback. The game joins the LAN group on the interface
 * of the default route, which a VPN's TUN often owns, and multicast sent on this machine then reaches nothing.
 */
public final class Lan {
    static final String GROUP = "224.0.2.60";
    private static final String SOCKET = "java/net/MulticastSocket";

    /** Announcements by text, with the local address each came from first. */
    private static final Map<String, InetAddress> seen = new HashMap<>();

    private Lan() {}

    /**
     * The game's LAN detector with its `joinGroup` and `receive` going through this class, or null for any other
     * class. Found by the group address and the socket calls, since the class name is obfuscated in most versions.
     */
    static byte[] apply(byte[] original) {
        if (!contains(original, GROUP) || !contains(original, SOCKET)) return null;
        ClassReader reader = new ClassReader(original);
        boolean[] patched = {false};
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
        reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(
                    int access, String name, String descriptor, String signature, String[] exceptions) {
                return new MethodVisitor(Opcodes.ASM9, super.visitMethod(access, name, descriptor, signature, exceptions)) {
                    @Override
                    public void visitMethodInsn(int opcode, String owner, String method, String desc, boolean itf) {
                        String hook = opcode == Opcodes.INVOKEVIRTUAL && owner.equals(SOCKET)
                                ? hook(method + desc)
                                : null;
                        if (hook == null) {
                            super.visitMethodInsn(opcode, owner, method, desc, itf);
                            return;
                        }
                        patched[0] = true;
                        bridge(this, hook);
                    }
                };
            }
        }, 0);
        return patched[0] ? writer.toByteArray() : null;
    }

    private static String hook(String call) {
        switch (call) {
            case "joinGroup(Ljava/net/InetAddress;)V":
                return "join";
            case "receive(Ljava/net/DatagramPacket;)V":
                return "receive";
            default:
                return null;
        }
    }

    /**
     * `[socket, argument]` on the stack becomes `Lan.method(socket, argument)`, found through the system class
     * loader like the authlib hooks. A method handle, unlike reflection, lets the game's own `catch` see the
     * socket's timeout as it is.
     */
    private static void bridge(MethodVisitor code, String method) {
        code.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/ClassLoader", "getSystemClassLoader", "()Ljava/lang/ClassLoader;", false);
        code.visitLdcInsn(Lan.class.getName());
        code.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/ClassLoader", "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", false);
        code.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/invoke/MethodHandles", "publicLookup", "()Ljava/lang/invoke/MethodHandles$Lookup;", false);
        code.visitInsn(Opcodes.SWAP);
        code.visitLdcInsn(method);
        code.visitInsn(Opcodes.ICONST_2);
        code.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/invoke/MethodType", "genericMethodType", "(I)Ljava/lang/invoke/MethodType;", false);
        code.visitFieldInsn(Opcodes.GETSTATIC, "java/lang/Void", "TYPE", "Ljava/lang/Class;");
        code.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/invoke/MethodType", "changeReturnType", "(Ljava/lang/Class;)Ljava/lang/invoke/MethodType;", false);
        code.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/invoke/MethodHandles$Lookup", "findStatic", "(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/MethodHandle;", false);
        code.visitInsn(Opcodes.DUP_X2);
        code.visitInsn(Opcodes.POP);
        code.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/invoke/MethodHandle", "invokeExact", "(Ljava/lang/Object;Ljava/lang/Object;)V", false);
    }

    /** The game's own join, then loopback too, where Riven's announcements always arrive. */
    public static void join(Object socket, Object group) throws IOException {
        MulticastSocket multicast = (MulticastSocket) socket;
        multicast.joinGroup((InetAddress) group);
        try {
            NetworkInterface loopback = NetworkInterface.getByInetAddress(InetAddress.getLoopbackAddress());
            if (loopback != null) multicast.joinGroup(new InetSocketAddress((InetAddress) group, 0), loopback);
        } catch (IOException | RuntimeException e) {
            // Already joined there when loopback is the default route, or no loopback multicast: the game's join stands.
        }
    }

    /**
     * The next announcement, skipping the copy of one this machine already sent from another of its addresses:
     * Riven announces on every interface, and the game now hears two of them.
     */
    public static void receive(Object socket, Object packet) throws IOException {
        DatagramPacket datagram = (DatagramPacket) packet;
        int room = datagram.getLength();
        while (true) {
            datagram.setLength(room);
            ((MulticastSocket) socket).receive(datagram);
            if (!repeated(datagram)) return;
        }
    }

    static boolean repeated(DatagramPacket datagram) {
        InetAddress from = datagram.getAddress();
        if (from == null || !local(from)) return false;
        String text = new String(datagram.getData(), datagram.getOffset(), datagram.getLength(), StandardCharsets.UTF_8);
        synchronized (seen) {
            // Worlds come and go over a long session; a full map only forgets which address came first.
            if (seen.size() > 256) seen.clear();
            InetAddress first = seen.putIfAbsent(text, from);
            return first != null && !first.equals(from);
        }
    }

    private static boolean local(InetAddress address) {
        if (address.isLoopbackAddress()) return true;
        try {
            return NetworkInterface.getByInetAddress(address) != null;
        } catch (IOException e) {
            return false;
        }
    }

    private static boolean contains(byte[] bytes, String text) {
        byte[] needle = text.getBytes(StandardCharsets.ISO_8859_1);
        outer:
        for (int i = 0; i + needle.length <= bytes.length; i++) {
            for (int j = 0; j < needle.length; j++) if (bytes[i + j] != needle[j]) continue outer;
            return true;
        }
        return false;
    }
}
