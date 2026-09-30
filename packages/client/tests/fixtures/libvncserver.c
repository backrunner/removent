/* Independent optional interop fixture; build against LibVNCServer 0.9.15.
 * Only synthetic pixels/credentials, bound to IPv4 loopback. See docs review.
 */
#include <rfb/rfb.h>
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>

static char *pixels(int width, int height) {
    unsigned char *data = calloc((size_t)width * height, 4);
    if (!data) exit(2);
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            size_t offset = ((size_t)y * width + x) * 4;
            data[offset] = 123;
            data[offset + 1] = (unsigned char)y;
            data[offset + 2] = (unsigned char)x;
        }
    }
    return (char *)data;
}

static void pointer(int mask, int x, int y, rfbClientPtr client) {
    printf("POINTER %d %d %d ENCODING %d\n", mask, x, y, client->preferredEncoding);
    fflush(stdout);
}

static void key(rfbBool down, rfbKeySym key, rfbClientPtr client) {
    if (!down) return;
    if (key == 'c') rfbDoCopyRect(client->screen, 16, 16, 32, 32, 16, 16);
    if (key == 'r') {
        char *old = client->screen->frameBuffer;
        rfbNewFramebuffer(client->screen, pixels(80, 60), 80, 60, 8, 3, 4);
        free(old);
    }
}

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int port = atoi(argv[1]), version = atoi(argv[2]), auth = atoi(argv[3]);
    int library_argc = 1;
    rfbScreenInfoPtr screen = rfbGetScreen(&library_argc, argv, 64, 48, 8, 3, 4);
    if (!screen) return 2;
    screen->port = port;
    screen->ipv6port = -1;
    screen->listenInterface = htonl(INADDR_LOOPBACK);
    screen->protocolMinorVersion = version;
    screen->alwaysShared = TRUE;
    screen->cursor = rfbMakeXCursor(1, 1, " ", " ");
    screen->frameBuffer = pixels(64, 48);
    screen->ptrAddEvent = pointer;
    screen->kbdAddEvent = key;
    char *passwords[] = {"interop-password", NULL};
    if (auth) {
        screen->authPasswdData = passwords;
        screen->passwordCheck = rfbCheckPasswordByList;
    }
    rfbInitServer(screen);
    if (screen->listenSock == RFB_INVALID_SOCKET) return 3;
    puts("READY");
    fflush(stdout);
    while (rfbIsActive(screen)) rfbProcessEvents(screen, 10000);
    rfbScreenCleanup(screen);
    return 0;
}
