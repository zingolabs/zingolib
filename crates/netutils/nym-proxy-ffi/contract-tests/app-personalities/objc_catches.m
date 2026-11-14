#import "AppPersonalities.h"

#import <Foundation/Foundation.h>

bool app_personalities_objc_catches(void) {
    @try {
        @throw [NSException exceptionWithName:@"AppPersonalities"
                                       reason:@"the Objective-C personality routine"
                                     userInfo:nil];
    } @catch (NSException *) {
        return true;
    }
}
