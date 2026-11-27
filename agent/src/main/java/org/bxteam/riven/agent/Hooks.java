package org.bxteam.riven.agent;

import java.lang.reflect.Method;
import java.net.MalformedURLException;
import java.net.URI;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.KeyFactory;
import java.security.PublicKey;
import java.security.Signature;
import java.security.spec.X509EncodedKeySpec;
import java.util.Base64;

/** What patched authlib calls. Settings come from system properties, read when needed. */
public final class Hooks {
    /** The skins service, such as `https://api.bxteam.org/v1/skins`. */
    static final String API = "riven.skins.api";
    /** Its texture-signing key, an X.509 public key in base64. */
    static final String KEY = "riven.skins.key";
    /** `true` for an offline account signed in to BX ID: its `join` goes to Riven, not Mojang. */
    static final String JOIN = "riven.skins.join";

    private static final String SESSION_HOST = "sessionserver.mojang.com";
    private static final String SESSION_PATH = "/session/minecraft/";

    private static String parsedFrom;
    private static PublicKey key;

    private Hooks() {}

    /** What patched authlib calls, by reflection: `valid` is authlib's own verdict. */
    public static Object signature(Object valid, Object property) {
        return signature(Boolean.TRUE.equals(valid), property);
    }

    public static Object domain(Object allowed, Object url) {
        return domain(Boolean.TRUE.equals(allowed), url instanceof String ? (String) url : null);
    }

    public static Object url(Object url, Object unused) {
        return url instanceof URL ? url((URL) url) : url;
    }

    /** A `textures` property authlib found unsigned by Mojang may be signed by Riven. */
    public static boolean signature(boolean valid, Object property) {
        if (valid || property == null) return valid;
        try {
            PublicKey ours = key();
            if (ours == null || !"textures".equals(read(property, "getName", "name"))) return false;
            String value = read(property, "getValue", "value");
            String signature = read(property, "getSignature", "signature");
            if (value == null || signature == null) return false;
            Signature check = Signature.getInstance("SHA1withRSA");
            check.initVerify(ours);
            check.update(value.getBytes(StandardCharsets.UTF_8));
            return check.verify(Base64.getDecoder().decode(signature));
        } catch (Exception e) {
            return false;
        }
    }

    /** Textures may also come from the skins service's own host. */
    public static boolean domain(boolean allowed, String url) {
        if (allowed || url == null) return allowed;
        URI api = api();
        if (api == null || api.getHost() == null) return false;
        try {
            URI uri = url.contains("://") ? new URI(url) : new URI("https://" + url);
            return api.getHost().equalsIgnoreCase(uri.getHost())
                    && (uri.getScheme().equals("https") || uri.getScheme().equals(api.getScheme()));
        } catch (Exception e) {
            return false;
        }
    }

    /**
     * Sends to Riven what Mojang cannot answer: `hasJoined` (Riven asks Mojang for everyone else), `join` for an
     * offline account, and the profile of an offline UUID. A Microsoft account's `join` stays with Mojang.
     */
    public static URL url(URL url) {
        if (url == null || !SESSION_HOST.equalsIgnoreCase(url.getHost())) return url;
        String path = url.getPath();
        if (path == null || !path.startsWith(SESSION_PATH)) return url;
        String rest = path.substring(SESSION_PATH.length());
        boolean ours = rest.equals("hasJoined")
                || (rest.equals("join") && Boolean.getBoolean(JOIN))
                || (rest.startsWith("profile/") && offline(rest.substring("profile/".length())));
        URI api = api();
        if (!ours || api == null) return url;
        try {
            String query = url.getQuery() == null ? "" : "?" + url.getQuery();
            return new URL(api.toString().replaceAll("/+$", "") + SESSION_PATH + rest + query);
        } catch (MalformedURLException e) {
            return url;
        }
    }

    /** Offline-mode UUIDs are version 3 (from the name); Mojang's are version 4. */
    static boolean offline(String uuid) {
        String hex = uuid.replace("-", "");
        return hex.length() == 32 && hex.matches("[0-9a-fA-F]+") && hex.charAt(12) == '3';
    }

    private static URI api() {
        String api = System.getProperty(API);
        if (api == null || api.isEmpty()) return null;
        try {
            return new URI(api);
        } catch (Exception e) {
            return null;
        }
    }

    private static synchronized PublicKey key() throws Exception {
        String encoded = System.getProperty(KEY);
        if (encoded == null || encoded.isEmpty()) return null;
        if (!encoded.equals(parsedFrom)) {
            key = KeyFactory.getInstance("RSA").generatePublic(new X509EncodedKeySpec(Base64.getDecoder().decode(encoded)));
            parsedFrom = encoded;
        }
        return key;
    }

    /** Property is a class with getters up to authlib 5 and a record after. */
    private static String read(Object property, String getter, String accessor) throws Exception {
        Method method;
        try {
            method = property.getClass().getMethod(getter);
        } catch (NoSuchMethodException e) {
            method = property.getClass().getMethod(accessor);
        }
        return (String) method.invoke(property);
    }
}
