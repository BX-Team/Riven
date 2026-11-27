package org.bxteam.riven.agent;

import java.util.ArrayList;
import java.util.List;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.Type;

/**
 * Finds the hook points by method name and descriptor anywhere in authlib, since the classes holding them moved
 * between versions while the methods kept their shape.
 */
final class Patch {
    static final String AUTHLIB = "com/mojang/authlib/";
    private static final String HOOKS = "org/bxteam/riven/agent/Hooks";

    /** The class after patching, or null when nothing in it is a hook point. */
    final byte[] bytes;
    /** `method kind` for every method patched. */
    final List<String> hooks;

    private Patch(byte[] bytes, List<String> hooks) {
        this.bytes = bytes;
        this.hooks = hooks;
    }

    enum Kind {
        SIGNATURE,
        DOMAIN,
        URL
    }

    static Kind kind(String name, String descriptor) {
        switch (name + descriptor) {
            case "isSignatureValid(Ljava/security/PublicKey;)Z":
            case "validateProperty(Lcom/mojang/authlib/properties/Property;)Z":
                return Kind.SIGNATURE;
            case "isWhitelistedDomain(Ljava/lang/String;)Z":
            case "isAllowedTextureDomain(Ljava/lang/String;)Z":
                return Kind.DOMAIN;
            case "createUrlConnection(Ljava/net/URL;)Ljava/net/HttpURLConnection;":
                return Kind.URL;
            default:
                return null;
        }
    }

    static Patch apply(byte[] original) {
        ClassReader reader = new ClassReader(original);
        // Only straight-line code goes in, so the frames stay valid and nothing needs the class hierarchy.
        ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
        List<String> hooks = new ArrayList<>();
        reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(
                    int access, String name, String descriptor, String signature, String[] exceptions) {
                MethodVisitor next = super.visitMethod(access, name, descriptor, signature, exceptions);
                Kind kind = kind(name, descriptor);
                if (kind == null || (access & Opcodes.ACC_ABSTRACT) != 0) return next;
                hooks.add(name + " " + kind.name().toLowerCase());
                // The one argument: the property's own `this`, else the first parameter.
                int slot = kind == Kind.SIGNATURE && name.equals("isSignatureValid")
                        ? 0
                        : (access & Opcodes.ACC_STATIC) != 0 ? 0 : 1;
                return new Hook(next, kind, slot);
            }
        }, 0);
        return new Patch(hooks.isEmpty() ? null : writer.toByteArray(), hooks);
    }

    private static final class Hook extends MethodVisitor {
        private final Kind kind;
        private final int slot;

        Hook(MethodVisitor next, Kind kind, int slot) {
            super(Opcodes.ASM9, next);
            this.kind = kind;
            this.slot = slot;
        }

        @Override
        public void visitCode() {
            super.visitCode();
            if (kind != Kind.URL) return;
            super.visitVarInsn(Opcodes.ALOAD, slot);
            super.visitInsn(Opcodes.ACONST_NULL);
            bridge("url");
            super.visitTypeInsn(Opcodes.CHECKCAST, "java/net/URL");
            super.visitVarInsn(Opcodes.ASTORE, slot);
        }

        @Override
        public void visitInsn(int opcode) {
            // authlib's answer is on the stack; the hook gets it and may only turn a refusal into a yes.
            if (opcode == Opcodes.IRETURN && kind != Kind.URL) {
                super.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/Boolean", "valueOf", "(Z)Ljava/lang/Boolean;", false);
                super.visitVarInsn(Opcodes.ALOAD, slot);
                bridge(kind == Kind.SIGNATURE ? "signature" : "domain");
                super.visitTypeInsn(Opcodes.CHECKCAST, "java/lang/Boolean");
                super.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Boolean", "booleanValue", "()Z", false);
            }
            super.visitInsn(opcode);
        }

        /**
         * `[a, b]` on the stack becomes `Hooks.method(a, b)`, found through the system class loader: game loaders
         * such as Fabric's Knot refuse to hand authlib a class from the agent's jar, while `java.*` reaches them all.
         */
        private void bridge(String method) {
            super.visitInsn(Opcodes.ICONST_2);
            super.visitTypeInsn(Opcodes.ANEWARRAY, "java/lang/Object");
            super.visitInsn(Opcodes.DUP_X2);
            super.visitInsn(Opcodes.DUP_X2);
            super.visitInsn(Opcodes.SWAP);
            super.visitInsn(Opcodes.ICONST_1);
            super.visitInsn(Opcodes.SWAP);
            super.visitInsn(Opcodes.AASTORE);
            super.visitInsn(Opcodes.ICONST_0);
            super.visitInsn(Opcodes.SWAP);
            super.visitInsn(Opcodes.AASTORE);
            super.visitMethodInsn(Opcodes.INVOKESTATIC, "java/lang/ClassLoader", "getSystemClassLoader", "()Ljava/lang/ClassLoader;", false);
            super.visitLdcInsn(HOOKS.replace('/', '.'));
            super.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/ClassLoader", "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", false);
            super.visitLdcInsn(method);
            super.visitInsn(Opcodes.ICONST_2);
            super.visitTypeInsn(Opcodes.ANEWARRAY, "java/lang/Class");
            for (int i = 0; i < 2; i++) {
                super.visitInsn(Opcodes.DUP);
                super.visitInsn(i == 0 ? Opcodes.ICONST_0 : Opcodes.ICONST_1);
                super.visitLdcInsn(Type.getType(Object.class));
                super.visitInsn(Opcodes.AASTORE);
            }
            super.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/Class", "getMethod", "(Ljava/lang/String;[Ljava/lang/Class;)Ljava/lang/reflect/Method;", false);
            super.visitInsn(Opcodes.SWAP);
            super.visitInsn(Opcodes.ACONST_NULL);
            super.visitInsn(Opcodes.SWAP);
            super.visitMethodInsn(Opcodes.INVOKEVIRTUAL, "java/lang/reflect/Method", "invoke", "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;", false);
        }
    }
}
